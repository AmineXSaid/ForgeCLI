//! Slash commands outside the full-screen UI: a prompt that starts with
//! `/name` is a built-in command, a custom command, a skill, or an MCP prompt.
//!
//! A word that looks like a path (`/usr/bin/env ...`) is an ordinary prompt.
//! An unknown command is reported without calling the model.

use std::path::Path;
use std::sync::Arc;

use forge_agents::{CommandDef, SkillDef};

/// Built-ins available in every mode: name and description.
pub const BUILTIN: &[(&str, &str)] = &[
    ("compact", "Summarize the conversation to free context (optional: what to keep)"),
    ("clear", "Start the conversation over"),
    ("cost", "Show this session's token use and cost"),
    ("help", "List the slash commands"),
];

#[derive(Debug, Clone, PartialEq)]
pub enum Slash {
    /// Not a command: send the text as it is.
    NotACommand,
    /// Send this prompt to the model.
    Prompt(String),
    Compact(Option<String>),
    Clear,
    Cost,
    /// Answer locally, without the model.
    Local(String),
    /// An error for the person (unknown command, failed expansion).
    Error(String),
}

/// The text of a prompt that may be a slash command (a plain text prompt starting with `/`).
pub fn command_text(content: &forge_types::MessageContent) -> Option<String> {
    let text = match content {
        forge_types::MessageContent::Text(t) => t.clone(),
        forge_types::MessageContent::Blocks(b) if b.len() == 1 => b[0].as_text()?.to_string(),
        _ => return None,
    };
    text.trim_start().starts_with('/').then_some(text)
}

pub struct SlashContext<'a> {
    pub commands: &'a [CommandDef],
    pub skills: &'a [SkillDef],
    pub mcp: Option<Arc<forge_mcp::McpManager>>,
    pub cwd: &'a Path,
}

impl SlashContext<'_> {
    pub fn help(&self) -> String {
        let mut lines: Vec<String> = BUILTIN.iter().map(|(n, d)| format!("/{n} - {d}")).collect();
        for c in self.commands {
            let hint = c.argument_hint.as_deref().map(|h| format!(" {h}")).unwrap_or_default();
            lines.push(format!("/{}{hint} - {} ({})", c.name, c.description, c.source));
        }
        for s in self.skills.iter().filter(|s| s.user_invocable) {
            lines.push(format!("/{} - {} (skill)", s.name, s.description));
        }
        for p in self.mcp.as_ref().map(|m| m.prompt_names()).unwrap_or_default() {
            lines.push(format!("/{p} (MCP prompt)"));
        }
        lines.join("\n")
    }

    pub async fn dispatch(&self, text: &str) -> Slash {
        let trimmed = text.trim_start();
        let Some(rest) = trimmed.strip_prefix('/') else { return Slash::NotACommand };
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (rest.trim_end(), ""),
        };
        if name.is_empty() || name.contains('/') || name.contains('\\') {
            return Slash::NotACommand;
        }
        match name {
            "compact" => return Slash::Compact(Some(args.to_string()).filter(|a| !a.is_empty())),
            "clear" | "reset" | "new" => return Slash::Clear,
            "cost" => return Slash::Cost,
            "help" => return Slash::Local(self.help()),
            _ => {}
        }
        if let Some(c) = self.commands.iter().find(|c| c.name == name) {
            return match forge_agents::commands::expand(c, args, self.cwd).await {
                Ok(p) => Slash::Prompt(p),
                Err(e) => Slash::Error(e),
            };
        }
        if let Some(s) = self.skills.iter().find(|s| s.user_invocable && s.name == name) {
            return Slash::Prompt(forge_agents::skills::skill_prompt(s, args));
        }
        if let Some(m) = &self.mcp {
            if let Some(r) = m.get_prompt(name, args).await {
                return match r {
                    Ok(p) => Slash::Prompt(p),
                    Err(e) => Slash::Error(format!("/{name}: {e}")),
                };
            }
        }
        Slash::Error(format!("Unknown command /{name}. /help lists the commands."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_commands_skills_and_paths() {
        let d = tempfile::tempdir().unwrap();
        let cmd = forge_agents::commands::parse_command(
            "review",
            "---\ndescription: Review\n---\nReview $ARGUMENTS",
            Path::new("x"),
            "project",
        );
        let skill = forge_agents::skills::parse_skill(
            d.path(),
            "---\nname: pdf\ndescription: PDFs\n---\nUse pdftotext.",
            "user",
        )
        .unwrap();
        let ctx = SlashContext { commands: &[cmd], skills: &[skill], mcp: None, cwd: d.path() };
        assert_eq!(ctx.dispatch("hello").await, Slash::NotACommand);
        assert_eq!(ctx.dispatch("/usr/bin/env python").await, Slash::NotACommand);
        assert_eq!(ctx.dispatch("/review src/lib.rs").await, Slash::Prompt("Review src/lib.rs".into()));
        assert!(
            matches!(ctx.dispatch("/pdf a.pdf").await, Slash::Prompt(p) if p.contains("Use pdftotext.") && p.ends_with("ARGUMENTS: a.pdf"))
        );
        assert_eq!(
            ctx.dispatch("/compact keep the API notes").await,
            Slash::Compact(Some("keep the API notes".into()))
        );
        assert_eq!(ctx.dispatch("/compact").await, Slash::Compact(None));
        assert_eq!(ctx.dispatch("/clear").await, Slash::Clear);
        assert!(
            matches!(ctx.dispatch("/help").await, Slash::Local(h) if h.contains("/review - Review (project)") && h.contains("/pdf - PDFs (skill)"))
        );
        assert!(matches!(ctx.dispatch("/nope").await, Slash::Error(e) if e.contains("Unknown command /nope")));
    }
}
