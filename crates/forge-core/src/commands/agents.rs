//! `/agents`: list the subagents, or create one:
//!
//! `/agents create <name> --description <text> [--prompt <text>] [--tools a,b]
//! [--model <model|inherit>] [--scope project|user]`
//!
//! It writes `<name>.md` in the format `forge_agents::parse_agent_markdown`
//! reads (project: `.forge/agents/`, user: the config directory's `agents/`),
//! then reloads, so the Task tool offers it at once. The TUI's agents screen
//! is a wizard for the same form.

use std::fmt::Write as _;
use std::path::PathBuf;

use super::run::{err, ok};
use super::screens::{Field, Form, Row, RowAction, Screen, Span, Tone};
use super::Exec;
use crate::driver::Driver;

const USAGE: &str = "Usage: /agents create <name> --description <text> [--prompt <text>] [--tools a,b] [--model <model|inherit>] [--scope project|user]";

/// Model choices the wizard offers (any model id works in the argument form).
const MODELS: &[&str] = &["inherit", "opus", "sonnet", "haiku"];

fn listing(d: &Driver) -> String {
    let mut s = String::from("Subagents:\n");
    for a in &d.catalog.agents {
        let tools = a.tools.as_ref().map(|t| t.join(", ")).unwrap_or_else(|| "all tools".into());
        let _ = writeln!(s, "  {} ({:?}) - {} [{}]", a.name, a.source, a.description, tools);
    }
    s.push_str(
        "\nTo add one, ask Forge to create it, use /agents create <name> --description <text>, or write \
         .forge/agents/<name>.md (a `---` header with name, description and optional tools and model, then its \
         instructions).",
    );
    s
}

/// Where an agent of `scope` is written.
fn dir(d: &Driver, scope: &str) -> PathBuf {
    match scope {
        // Beside the user settings file: the config directory.
        "user" => d
            .info
            .user_settings
            .parent()
            .map(|p| p.join("agents"))
            .unwrap_or_else(|| forge_config::forge_home().join("agents")),
        _ => d.info.cwd.join(".forge/agents"),
    }
}

/// The agent file's text.
fn markdown(name: &str, description: &str, tools: &[String], model: &str, prompt: &str) -> String {
    let mut s = format!("---\nname: {name}\ndescription: {}\n", description.replace('\n', " "));
    if !tools.is_empty() {
        let _ = writeln!(s, "tools: {}", tools.join(", "));
    }
    if !model.is_empty() && model != "inherit" {
        let _ = writeln!(s, "model: {model}");
    }
    let _ = write!(s, "---\n\n{}\n", prompt.trim());
    s
}

async fn create(d: &mut Driver, words: &[String]) -> Exec {
    let mut name = None;
    let (mut description, mut prompt, mut model, mut scope) =
        (String::new(), String::new(), String::new(), "project".to_string());
    let mut tools: Vec<String> = vec![];
    let mut it = words.iter();
    while let Some(w) = it.next() {
        let mut value = || it.next().cloned();
        match w.as_str() {
            "--description" => description = value().unwrap_or_default(),
            "--prompt" => prompt = value().unwrap_or_default(),
            "--model" => model = value().unwrap_or_default(),
            "--scope" => scope = value().unwrap_or_default(),
            "--tools" => {
                tools = value()
                    .unwrap_or_default()
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            }
            other if name.is_none() && !other.starts_with("--") => name = Some(other.to_string()),
            other => return err(format!("Unknown option {other}. {USAGE}")),
        }
    }
    let Some(name) = name else { return err(USAGE) };
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return err("An agent's name is lowercase letters, digits and dashes (e.g. code-reviewer).");
    }
    if description.trim().is_empty() {
        return err(format!("An agent needs a description: when Forge should use it. {USAGE}"));
    }
    if !matches!(scope.as_str(), "project" | "user") {
        return err("--scope takes project or user.");
    }
    let known = d.engine.tools().names();
    let unknown: Vec<&String> = tools.iter().filter(|t| !known.contains(t)).collect();
    if !unknown.is_empty() {
        return err(format!(
            "Unknown tool(s): {}. Tools: {}.",
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            known.join(", ")
        ));
    }
    if !model.is_empty()
        && model != "inherit"
        && forge_api::models::model_info(&forge_api::resolve_model(&model)).is_none()
    {
        return err(format!("Unknown model {model}. Use inherit, an alias (opus, sonnet, haiku) or a model id."));
    }
    if prompt.trim().is_empty() {
        prompt = format!("You are a subagent for this task: {}\n\nDo the task you are given, then report back what you found or did, briefly.", description.trim());
    }
    let dir = dir(d, &scope);
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        return err(format!("{} already exists. Edit it, or pick another name.", path.display()));
    }
    let text = markdown(&name, description.trim(), &tools, &model, &prompt);
    // Check it reads back before writing it.
    if let Err(e) = forge_agents::parse_agent_markdown(&text, forge_agents::AgentSource::Project) {
        return err(format!("Could not create the agent: {e}"));
    }
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &text)) {
        return err(format!("Could not write {}: {e}", path.display()));
    }
    let note = format!("Created the {name} agent in {}.", path.display());
    if !d.can_switch() {
        return ok(format!("{note} It is available in new sessions."));
    }
    match super::switching::reload(d, "skills").await {
        Exec::Local { text, is_error: false } => ok(format!("{note} {text}")),
        Exec::Local { text, .. } => err(format!("{note} {text}")),
        other => other,
    }
}

pub(super) async fn run(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return ok(listing(d));
    }
    let Some(words) = shlex::split(args) else { return err(USAGE) };
    match words.first().map(String::as_str) {
        Some("create") => create(d, &words[1..]).await,
        _ => err(USAGE),
    }
}

/// The wizard: name, description, instructions, tools, model, scope.
fn create_form(d: &Driver) -> Form {
    let tools: Vec<String> = d.engine.tools().names().into_iter().filter(|n| !n.starts_with("mcp__")).collect();
    Form {
        title: "Create an agent".into(),
        fields: vec![
            Field::text("Name (e.g. code-reviewer)", ""),
            Field::text("Description: when to use it", ""),
            Field::text("Instructions (empty: from the description)", ""),
            Field::multi("Tools (none picked: all)", tools),
            Field::choice("Model", MODELS),
            Field::choice("Save in", &["project", "user"]),
        ],
        template: "/agents create {0} --description {1} --prompt {2} --tools {3} --model {4} --scope {5}".into(),
    }
}

/// The agents screen: each agent, and "Create an agent…".
pub(super) fn screen(d: &Driver) -> Screen {
    let mut rows = vec![
        Row {
            spans: vec![Span { text: "+ Create an agent…".into(), tone: Tone::Accent }],
            action: Some(RowAction::Form(create_form(d))),
        },
        Row::blank(),
    ];
    for a in &d.catalog.agents {
        let tools = a.tools.as_ref().map(|t| t.join(", ")).unwrap_or_else(|| "all tools".into());
        let model = a.model.as_deref().unwrap_or("inherit");
        rows.push(Row {
            spans: vec![
                Span { text: a.name.clone(), tone: Tone::Bold },
                Span { text: format!("  {:?} · {model} · {tools}", a.source), tone: Tone::Dim },
            ],
            action: None,
        });
        rows.push(Row::text(format!("  {}", a.description), Tone::Plain));
    }
    Screen { title: "Agents".into(), rows }
}
