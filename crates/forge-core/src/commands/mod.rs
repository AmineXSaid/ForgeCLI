//! Built-in slash commands: one registry for every front end (contract C17).
//!
//! A message that starts with `/name` is one of these, in this order:
//! 1. a built-in command from [`BUILTINS`];
//! 2. a custom command (`commands/*.md`);
//! 3. a skill, or a chain of up to six (`/a /b text`);
//! 4. an MCP prompt (`/mcp__server__prompt`).
//!
//! A name containing `/`, or matching an existing root path (`/tmp is full`),
//! is an ordinary prompt. Anything else is `Unknown command: /name`.
//!
//! [`BUILTINS`] is the only list of built-ins. `/help`, `system/init`
//! `slash_commands` and the stream-json `initialize` `commands` are all
//! generated from it.

mod feedback;
mod importing;
mod looping;
mod run;
mod session;
mod settings;
mod switching;

pub(crate) use looping::scheduled_prompt;
pub use run::{execute, Exec};
pub(crate) use session::side_request_with;
pub use session::{clean_title, render_conversation};
pub use settings::Scope;

use std::path::Path;
use std::sync::Arc;

use forge_agents::{CommandDef, SkillDef};
use serde_json::{json, Value};

/// Where a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// `-p` with text or json output: one prompt, then exit.
    Print,
    /// `-p --input-format stream-json`: an SDK host drives the session.
    Stream,
    /// The line REPL.
    Repl,
    /// The full-screen UI.
    Tui,
}

/// A set of [`Surface`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surfaces(u8);

impl Surfaces {
    pub const ALL: Surfaces = Surfaces(0b1111);
    pub const INTERACTIVE: Surfaces = Surfaces(0b1100);
    pub const TUI: Surfaces = Surfaces(0b1000);

    pub fn has(self, s: Surface) -> bool {
        let bit = match s {
            Surface::Print => 0b0001,
            Surface::Stream => 0b0010,
            Surface::Repl => 0b0100,
            Surface::Tui => 0b1000,
        };
        self.0 & bit != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandSpec {
    pub id: Builtin,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Argument hint, e.g. `[instructions]`.
    pub args: &'static str,
    pub description: &'static str,
    pub surfaces: Surfaces,
    /// Runs at once, even while a turn is in progress (TUI and stream-json).
    pub immediate: bool,
}

/// The command table and the `Builtin` enum, from one list, so a command
/// can't be in one and missing from the other.
macro_rules! builtins {
    ($( ($id:ident, $($rest:tt)*) ),* $(,)?) => {
        /// A built-in command.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Builtin { $($id),* }

        /// Every built-in command, alphabetically.
        pub static BUILTINS: &[CommandSpec] = &[ $( cmd!($id, $($rest)*) ),* ];
    };
}

macro_rules! cmd {
    ($id:ident, $name:literal, [$($alias:literal),*], $args:literal, $desc:literal) => {
        cmd!($id, $name, [$($alias),*], $args, $desc, Surfaces::ALL, false)
    };
    ($id:ident, $name:literal, [$($alias:literal),*], $args:literal, $desc:literal, $surf:expr, $imm:expr) => {
        CommandSpec {
            id: Builtin::$id,
            name: $name,
            aliases: &[$($alias),*],
            args: $args,
            description: $desc,
            surfaces: $surf,
            immediate: $imm,
        }
    };
}

builtins! {
    (AddDir, "add-dir", [], "<path> [--save]", "Add a working directory for this session"),
    (Advisor, "advisor", [], "[model|off]", "Let Forge consult a second model for advice at key moments"),
    (Agents, "agents", [], "", "List subagents, and how to add your own"),
    (
        Autocompact,
        "autocompact",
        [],
        "[on|off|auto|<tokens>]",
        "Show or set when the conversation is compacted automatically"
    ),
    (Branch, "branch", [], "[name]", "Branch the conversation into a new session; the original stays as it was"),
    (
        Btw,
        "btw",
        [],
        "[question]",
        "Ask a side question without tools; the conversation stays as it was",
        Surfaces::ALL,
        true
    ),
    (Cd, "cd", [], "<directory>", "Move this conversation to another directory"),
    (Clear, "clear", ["reset", "new"], "[name]", "Start a new conversation; the current one stays resumable"),
    (Compact, "compact", [], "[instructions]", "Free context by summarizing the conversation so far"),
    (Config, "config", ["settings"], "[key=value ...]", "Show the settings, or change them with key=value"),
    (Context, "context", [], "[all]", "Show what fills the context window", Surfaces::ALL, true),
    (Debug, "debug", [], "[description]", "Turn on debug logging, and have Forge read the log to find a problem"),
    (Diff, "diff", [], "", "Show uncommitted changes, and the files each prompt changed"),
    (Doctor, "doctor", ["checkup"], "", "Check the installation and this session's setup"),
    (Effort, "effort", [], "[low|medium|high|xhigh|max|auto]", "Show or set the model's reasoning effort"),
    (Exit, "exit", ["quit"], "", "Exit Forge"),
    (Export, "export", [], "[file]", "Export the conversation as plain text"),
    (Fast, "fast", [], "[on|off]", "Turn fast mode on or off, where the model offers it"),
    (
        Feedback,
        "feedback",
        ["bug", "share"],
        "[description]",
        "Save a bug-report bundle on this machine (nothing is uploaded)"
    ),
    (Goal, "goal", [], "[condition|clear]", "Set a goal Forge keeps working toward until a check finds it met"),
    (Help, "help", [], "", "Show help and the available commands"),
    (Hooks, "hooks", [], "", "View the configured hooks"),
    (
        Import,
        "import",
        [],
        "[codex|gemini|cursor] [--yes]",
        "Bring MCP servers and instructions over from other coding agents"
    ),
    (
        Loop,
        "loop",
        [],
        "[interval] [prompt]",
        "Run a prompt on a schedule, or let Forge pace it; /tasks lists them"
    ),
    (Mcp, "mcp", [], "", "Show MCP server status", Surfaces::ALL, true),
    (Memory, "memory", [], "", "List the memory files in use (FORGE.md, AGENTS.md)"),
    (Model, "model", [], "[model]", "Show the models, or switch to one"),
    (OutputStyle, "output-style", [], "[style]", "Show the output styles, or switch to one"),
    (
        Permissions,
        "permissions",
        ["allowed-tools"],
        "[add|remove ...]",
        "Show the permission rules and working directories, or change the rules"
    ),
    (Plan, "plan", [], "[description]", "Enter plan mode; with a description, start planning it"),
    (Plugin, "plugin", [], "[list]", "List loaded plugins"),
    (Recap, "recap", [], "", "Summarize the session in one line"),
    (ReleaseNotes, "release-notes", [], "", "Show what changed in each version"),
    (ReloadPlugins, "reload-plugins", [], "", "Reload plugins without restarting"),
    (ReloadSkills, "reload-skills", [], "", "Reload skills, commands and agents without restarting"),
    (Rename, "rename", [], "[name]", "Rename this session (Forge suggests a name when you give none)"),
    (Resume, "resume", ["continue"], "[session]", "Resume another conversation from this directory"),
    (
        Rewind,
        "rewind",
        ["checkpoint", "undo"],
        "[<n> <both|conversation|code|summarize-from|summarize-to>]",
        "Go back to an earlier prompt: restore code, conversation or both, or summarize"
    ),
    (
        Sandbox,
        "sandbox",
        [],
        "[on|off|read-only|workspace-write]",
        "Show or change the OS sandbox for shell commands"
    ),
    (Skills, "skills", [], "", "List available skills"),
    (Status, "status", [], "", "Show version, model, session and setup status", Surfaces::ALL, true),
    (
        Tasks,
        "tasks",
        ["bashes"],
        "[stop <id>]",
        "View and stop background work in this session",
        Surfaces::ALL,
        true
    ),
    (
        Usage,
        "usage",
        ["cost", "stats"],
        "",
        "Show this session's cost, token use and activity",
        Surfaces::ALL,
        true
    ),
}

/// The built-in called `name` (or one of its aliases).
pub fn lookup(name: &str) -> Option<&'static CommandSpec> {
    BUILTINS.iter().find(|c| c.name == name || c.aliases.contains(&name))
}

/// What slash commands can reach besides the built-ins.
#[derive(Default)]
pub struct Catalog {
    pub commands: Vec<CommandDef>,
    pub skills: Vec<SkillDef>,
    pub styles: Vec<forge_agents::OutputStyle>,
    pub plugins: Vec<forge_agents::Plugin>,
    pub agents: Vec<forge_agents::AgentDef>,
    pub mcp: Option<Arc<forge_mcp::McpManager>>,
}

impl Catalog {
    /// Skills a person can type: those a built-in or custom command of the same name doesn't shadow.
    fn user_skills(&self) -> impl Iterator<Item = &SkillDef> {
        self.skills.iter().filter(|s| {
            s.user_invocable && lookup(&s.name).is_none() && !self.commands.iter().any(|c| c.name == s.name)
        })
    }

    fn mcp_prompts(&self) -> Vec<String> {
        self.mcp.as_ref().map(|m| m.prompt_names()).unwrap_or_default()
    }

    /// Command names for `system/init` on `surface`: built-ins, custom commands, skills, MCP prompts.
    pub fn names(&self, surface: Surface) -> Vec<String> {
        BUILTINS
            .iter()
            .filter(|c| c.surfaces.has(surface))
            .map(|c| c.name.to_string())
            .chain(self.commands.iter().map(|c| c.name.clone()))
            .chain(self.user_skills().map(|s| s.name.clone()))
            .chain(self.mcp_prompts())
            .collect()
    }

    /// The `initialize` response's `commands`: `{name, description, argumentHint}`.
    pub fn catalog_json(&self, surface: Surface) -> Vec<Value> {
        let row = |name: &str, desc: &str, hint: &str| json!({"name": name, "description": desc, "argumentHint": hint});
        BUILTINS
            .iter()
            .filter(|c| c.surfaces.has(surface))
            .map(|c| row(c.name, c.description, c.args))
            .chain(self.commands.iter().map(|c| row(&c.name, &c.description, c.argument_hint.as_deref().unwrap_or(""))))
            .chain(self.user_skills().map(|s| row(&s.name, &s.description, "")))
            .chain(self.mcp_prompts().iter().map(|p| row(p, "MCP prompt", "")))
            .collect()
    }

    /// `/help` text for `surface`.
    pub fn help(&self, surface: Surface) -> String {
        let mut lines = vec!["Commands:".to_string()];
        for c in BUILTINS.iter().filter(|c| c.surfaces.has(surface)) {
            let args = if c.args.is_empty() { String::new() } else { format!(" {}", c.args) };
            let aliases = if c.aliases.is_empty() {
                String::new()
            } else {
                format!(" (also {})", c.aliases.iter().map(|a| format!("/{a}")).collect::<Vec<_>>().join(", "))
            };
            lines.push(format!("  /{}{args} - {}{aliases}", c.name, c.description));
        }
        let custom: Vec<String> = self
            .commands
            .iter()
            .map(|c| {
                let hint = c.argument_hint.as_deref().map(|h| format!(" {h}")).unwrap_or_default();
                format!("  /{}{hint} - {} ({})", c.name, c.description, c.source)
            })
            .chain(self.user_skills().map(|s| {
                let tag = if s.source == "bundled" { "bundled skill" } else { "skill" };
                format!("  /{} - {} ({tag})", s.name, s.description)
            }))
            .chain(self.mcp_prompts().iter().map(|p| format!("  /{p} (MCP prompt)")))
            .collect();
        if !custom.is_empty() {
            lines.push(String::new());
            lines.push("Custom commands, skills and MCP prompts:".into());
            lines.extend(custom);
        }
        lines.push(String::new());
        lines.push("A message starting with /name runs a command; anything after the name is its arguments.".into());
        lines.join("\n")
    }
}

/// How a message is read.
#[derive(Debug, PartialEq)]
pub enum Invocation<'a> {
    NotACommand,
    Builtin {
        spec: &'static CommandSpec,
        args: &'a str,
    },
    Custom {
        def: &'a CommandDef,
        args: &'a str,
    },
    /// One or more skills (`/a /b text`), each given the same arguments.
    Skills {
        chain: Vec<&'a SkillDef>,
        args: &'a str,
    },
    McpPrompt {
        name: &'a str,
        args: &'a str,
    },
    Unknown(&'a str),
}

/// The most skills one message may load (`/a /b /c ... text`).
pub const MAX_SKILL_CHAIN: usize = 6;

fn split_name(rest: &str) -> (&str, &str) {
    match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim_start()),
        None => (rest.trim_end(), ""),
    }
}

/// Read `text` (a whole message). Commands are recognized only at the start.
pub fn parse<'a>(text: &'a str, cat: &'a Catalog) -> Invocation<'a> {
    let Some(rest) = text.trim_start().strip_prefix('/') else { return Invocation::NotACommand };
    let (name, args) = split_name(rest);
    let args = args.trim_end();
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Invocation::NotACommand;
    }
    if let Some(spec) = lookup(name) {
        return Invocation::Builtin { spec, args };
    }
    if let Some(def) = cat.commands.iter().find(|c| c.name == name) {
        return Invocation::Custom { def, args };
    }
    let skill = |n: &str| cat.skills.iter().find(|s| s.user_invocable && s.name == n);
    if let Some(first) = skill(name) {
        let mut chain = vec![first];
        let mut rest_args = args;
        while chain.len() < MAX_SKILL_CHAIN {
            let Some(next) = rest_args.strip_prefix('/') else { break };
            let (n, a) = split_name(next);
            match skill(n) {
                Some(s) => {
                    chain.push(s);
                    rest_args = a;
                }
                None => break,
            }
        }
        return Invocation::Skills { chain, args: rest_args };
    }
    if cat.mcp_prompts().iter().any(|p| p == name) {
        return Invocation::McpPrompt { name, args };
    }
    // "/tmp is full": a path, not a command.
    if Path::new(&format!("/{name}")).exists() {
        return Invocation::NotACommand;
    }
    Invocation::Unknown(name)
}

/// The text of a prompt that may be a slash command (a plain text prompt starting with `/`).
pub fn command_text(content: &forge_types::MessageContent) -> Option<String> {
    command_text_any(content).filter(|t| t.trim_start().starts_with('/'))
}

/// The text of a message that is a single piece of text.
pub fn command_text_any(content: &forge_types::MessageContent) -> Option<String> {
    match content {
        forge_types::MessageContent::Text(t) => Some(t.clone()),
        forge_types::MessageContent::Blocks(b) if b.len() == 1 => b[0].as_text().map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(dir: &Path) -> Catalog {
        let cmd = forge_agents::commands::parse_command(
            "review-pr",
            "---\ndescription: Review a PR\nargument-hint: <number>\n---\nReview PR $ARGUMENTS",
            Path::new("x"),
            "project",
        );
        let skill = |n: &str| {
            forge_agents::skills::parse_skill(
                dir,
                &format!("---\nname: {n}\ndescription: {n} things\n---\nUse {n}."),
                "user",
            )
            .unwrap()
        };
        Catalog { commands: vec![cmd], skills: vec![skill("pdf"), skill("xlsx"), skill("docx")], ..Default::default() }
    }

    #[test]
    fn registry_is_consistent() {
        let mut names: Vec<&str> = vec![];
        for c in BUILTINS {
            assert!(!c.description.is_empty() && c.description.len() < 80, "{}", c.name);
            for n in std::iter::once(&c.name).chain(c.aliases) {
                assert!(!names.contains(n), "duplicate command name /{n}");
                assert!(n.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-'), "/{n}");
                names.push(n);
            }
        }
        assert!(BUILTINS.windows(2).all(|w| w[0].name < w[1].name), "keep BUILTINS sorted");
        // Account and cloud commands are left out on purpose.
        for out in ["login", "logout", "upgrade", "teleport", "remote-control", "desktop", "mobile", "chrome"] {
            assert!(lookup(out).is_none(), "/{out} is out of scope");
        }
        assert_eq!(lookup("cost").unwrap().name, "usage");
        assert_eq!(lookup("quit").unwrap().id, Builtin::Exit);
    }

    #[test]
    fn parses_commands_skills_paths_and_unknowns() {
        let d = tempfile::tempdir().unwrap();
        let cat = catalog(d.path());
        assert_eq!(parse("hello /help", &cat), Invocation::NotACommand, "only at the start");
        assert_eq!(parse("/usr/bin/env python", &cat), Invocation::NotACommand);
        assert_eq!(parse("/tmp is full", &cat), Invocation::NotACommand, "an existing root path");
        assert!(
            matches!(parse("  /compact keep the API", &cat), Invocation::Builtin { spec, args: "keep the API" } if spec.id == Builtin::Compact)
        );
        assert!(matches!(parse("/cost", &cat), Invocation::Builtin { spec, args: "" } if spec.id == Builtin::Usage));
        assert!(matches!(parse("/review-pr 12", &cat), Invocation::Custom { args: "12", .. }));
        match parse("/pdf /xlsx /docx report.pdf and data.xlsx", &cat) {
            Invocation::Skills { chain, args } => {
                assert_eq!(chain.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["pdf", "xlsx", "docx"]);
                assert_eq!(args, "report.pdf and data.xlsx");
            }
            other => panic!("{other:?}"),
        }
        match parse("/pdf /not-a-skill x", &cat) {
            Invocation::Skills { chain, args } => assert_eq!((chain.len(), args), (1, "/not-a-skill x")),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse("/nope", &cat), Invocation::Unknown("nope"));
        assert_eq!(parse("/login", &cat), Invocation::Unknown("login"));
    }

    #[test]
    fn help_and_catalog_come_from_the_registry() {
        let d = tempfile::tempdir().unwrap();
        let cat = catalog(d.path());
        let help = cat.help(Surface::Repl);
        assert!(
            help.contains("  /usage - Show this session's cost, token use and activity (also /cost, /stats)"),
            "{help}"
        );
        assert!(
            help.contains("  /review-pr <number> - Review a PR (project)")
                && help.contains("  /pdf - pdf things (skill)")
        );
        let names = cat.names(Surface::Stream);
        assert!(names.contains(&"compact".to_string()) && names.contains(&"pdf".to_string()));
        let json = cat.catalog_json(Surface::Stream);
        assert!(json.iter().any(|c| c["name"] == "compact" && c["argumentHint"] == "[instructions]"));
    }
}
