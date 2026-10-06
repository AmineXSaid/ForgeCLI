//! What each built-in command does.

use std::fmt::Write as _;

use forge_types::MessageContent;
use serde_json::Value;

use super::{parse, Builtin, Invocation};
use crate::driver::Driver;

/// What a command asks the driver to do next.
#[derive(Debug, PartialEq)]
pub enum Exec {
    /// Send this to the model as the user's prompt.
    Submit(MessageContent),
    /// Answer locally; nothing goes to the model.
    Local { text: String, is_error: bool },
    /// End the session.
    Exit,
}

pub(super) fn ok(text: impl Into<String>) -> Exec {
    Exec::Local { text: text.into(), is_error: false }
}

pub(super) fn err(text: impl Into<String>) -> Exec {
    Exec::Local { text: text.into(), is_error: true }
}

/// Run the slash command in `text` (a message starting with `/`).
pub async fn execute(d: &mut Driver, text: &str) -> Exec {
    let cwd = d.info.cwd.clone();
    let (id, args) = match parse(text, &d.catalog) {
        Invocation::NotACommand => return Exec::Submit(MessageContent::Text(text.to_string())),
        Invocation::Unknown(name) => return err(format!("Unknown command: /{name}")),
        Invocation::Custom { def, args } => {
            return match forge_agents::commands::expand(def, args, &cwd).await {
                Ok(p) => Exec::Submit(MessageContent::Text(p)),
                Err(e) => err(e),
            }
        }
        Invocation::Skills { chain, args } => {
            let prompts: Vec<String> = chain.iter().map(|s| forge_agents::skills::skill_prompt(s, args)).collect();
            return Exec::Submit(MessageContent::Text(prompts.join("\n\n")));
        }
        Invocation::McpPrompt { name, args } => {
            let name = name.to_string();
            let Some(m) = d.catalog.mcp.clone() else { return err(format!("Unknown command: /{name}")) };
            return match m.get_prompt(&name, args).await {
                Some(Ok(p)) => Exec::Submit(MessageContent::Text(p)),
                Some(Err(e)) => err(format!("/{name}: {e}")),
                None => err(format!("Unknown command: /{name}")),
            };
        }
        Invocation::Builtin { spec, args } => (spec.id, args.to_string()),
    };
    let args = args.trim();
    match id {
        Builtin::Help => ok(d.catalog.help(d.surface)),
        Builtin::Exit => Exec::Exit,
        Builtin::Clear => super::switching::clear(d, args).await,
        Builtin::Compact => match d.engine.compact(Some(args).filter(|a| !a.is_empty())).await {
            Ok(info) => ok(format!("Compacted the conversation (about {} tokens before).", info.pre_tokens)),
            Err(e) => err(format!("Could not compact: {e}")),
        },
        Builtin::Usage => ok(usage(d)),
        Builtin::Status => ok(status(d)),
        Builtin::Skills => ok(skills(d)),
        Builtin::Agents => ok(agents(d)),
        Builtin::Hooks => ok(hooks(d)),
        Builtin::Memory => ok(memory(d)),
        Builtin::Doctor => doctor(d),
        Builtin::ReleaseNotes => ok(release_notes()),
        Builtin::Plugin => match args {
            "" | "list" => ok(plugins(d)),
            _ => err("Plugin marketplaces aren't supported. Load a plugin directory with --plugin-dir or the pluginDirs setting."),
        },
        Builtin::Mcp => match args {
            "" => ok(mcp(d)),
            _ => err("Usage: /mcp"),
        },
        Builtin::Tasks => tasks(d, args),
        Builtin::Model => super::settings::model(d, args),
        Builtin::Effort => super::settings::effort(d, args),
        Builtin::Fast => super::settings::fast(d, args),
        Builtin::Config => super::settings::config(d, args),
        Builtin::OutputStyle => super::settings::output_style(d, args),
        Builtin::Autocompact => super::settings::autocompact(d, args),
        Builtin::Sandbox => super::settings::sandbox(d, args),
        Builtin::Rename => super::session::rename(d, args).await,
        Builtin::Export => super::session::export(d, args),
        Builtin::Diff => match args {
            "" => super::session::diff(d),
            _ => err("Usage: /diff"),
        },
        Builtin::Context => super::session::context(d, args),
        Builtin::Debug => super::session::debug(d, args),
        Builtin::Permissions => super::session::permissions(d, args),
        Builtin::AddDir => super::session::add_dir(d, args),
        Builtin::Goal => super::session::goal(d, args),
        Builtin::Btw => super::session::btw(d, args).await,
        Builtin::Recap => match args {
            "" => super::session::recap(d).await,
            _ => err("Usage: /recap"),
        },
        Builtin::Plan => super::session::plan(d, args),
        Builtin::Rewind => super::switching::rewind(d, args).await,
        Builtin::Resume => super::switching::resume(d, args).await,
        Builtin::Branch => super::switching::branch(d, args).await,
        Builtin::Cd => super::switching::cd(d, args).await,
        Builtin::ReloadSkills | Builtin::ReloadPlugins => {
            if !d.can_switch() {
                return err("Reloading isn't available here.");
            }
            let what = if id == Builtin::ReloadPlugins { "plugins" } else { "skills" };
            super::switching::reload(d, what).await
        }
    }
}

pub(super) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(super) fn duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

fn usage(d: &Driver) -> String {
    let st = &d.engine.state;
    let tool_calls: usize = st.messages.iter().map(|m| m.tool_uses().count()).sum();
    let mut s = String::new();
    let _ = writeln!(s, "Total cost:     ${:.4}", st.total_cost_usd);
    let _ = writeln!(
        s,
        "Duration:       {} wall, {} API · {} model calls for {} prompts · {} tool calls in this conversation",
        duration(d.started.elapsed()),
        duration(d.activity.api_time),
        d.activity.turns,
        d.activity.prompts,
        tool_calls
    );
    if st.model_usage.is_empty() {
        let _ = writeln!(s, "Usage by model: none yet");
    } else {
        let _ = writeln!(s, "Usage by model:");
        for (model, u) in &st.model_usage {
            let n = |k: &str| thousands(u.get(k).and_then(Value::as_u64).unwrap_or(0));
            let _ = writeln!(
                s,
                "  {model}: {} input, {} output, {} cache read, {} cache write (${:.4})",
                n("inputTokens"),
                n("outputTokens"),
                n("cacheReadInputTokens"),
                n("cacheCreationInputTokens"),
                u.get("costUSD").and_then(Value::as_f64).unwrap_or(0.0)
            );
        }
    }
    let model = d.handle().model();
    let window = forge_api::models::model_info_or_default(&model).context_window;
    let _ = write!(s, "Context now:    about {} of {} tokens", thousands(st.context_tokens), thousands(window));
    s
}

fn status(d: &Driver) -> String {
    let h = d.handle();
    let rt = h.runtime.read().unwrap().clone();
    let mode = h.permissions.read().unwrap().mode;
    let info = &d.info;
    let mut s = String::new();
    let _ = writeln!(s, "ForgeCLI {}", crate::VERSION);
    let title = d.engine.transcript().title();
    let _ = writeln!(s, "Session:        {}{}", info.session_id, title.map(|t| format!(" ({t})")).unwrap_or_default());
    let _ = writeln!(s, "Directory:      {}", info.cwd.display());
    let dirs: Vec<String> =
        d.engine.tool_ctx().working_dirs.read().unwrap().iter().skip(1).map(|p| p.display().to_string()).collect();
    if !dirs.is_empty() {
        let _ = writeln!(s, "Also allowed:   {}", dirs.join(", "));
    }
    let _ = writeln!(
        s,
        "Model:          {} · effort {} · thinking {}",
        rt.model,
        rt.effort.as_deref().unwrap_or("default"),
        match rt.max_thinking_tokens {
            None => "default".to_string(),
            Some(0) => "off".to_string(),
            Some(n) => format!("{n} tokens"),
        }
    );
    let _ = writeln!(s, "Permissions:    {} mode", mode.as_str());
    let _ = writeln!(s, "Output style:   {}", info.init.output_style);
    let sandbox = match d.engine.tool_ctx().sandbox_policy() {
        Some(p) => format!("{} (network {})", p.mode.as_str(), if p.network { "on" } else { "off" }),
        None => "off".into(),
    };
    let _ = writeln!(s, "Sandbox:        {sandbox}");
    let _ = writeln!(s, "API:            {} via {}", info.init.api_key_source, d.engine.provider_name());
    let files: Vec<String> = info
        .settings
        .layers
        .iter()
        .map(|l| match &l.path {
            Some(p) => format!("{} ({})", p.display(), l.source.as_str()),
            None => l.source.as_str().to_string(),
        })
        .collect();
    let _ = writeln!(s, "Settings:       {}", if files.is_empty() { "none".into() } else { files.join(", ") });
    let memory = forge_config::load_memory(&info.cwd);
    let _ = writeln!(s, "Memory:         {} file(s)", memory.len());
    if let Some(m) = &d.catalog.mcp {
        let st = m.status();
        let count = |w: &str| st.iter().filter(|(_, s)| s == w).count();
        let _ = writeln!(
            s,
            "MCP servers:    {} connected, {} failed, {} disabled",
            count("connected"),
            count("failed"),
            count("disabled")
        );
    }
    let hooks = d.engine.hooks();
    let n: usize = hooks.config.events.values().map(|m| m.iter().map(|x| x.hooks.len()).sum::<usize>()).sum();
    let _ = write!(
        s,
        "Hooks:          {}",
        if hooks.disabled { "disabled".to_string() } else { format!("{n} configured") }
    );
    s
}

pub(super) fn tokens_of(text: &str) -> usize {
    text.len() / 4
}

fn skills(d: &Driver) -> String {
    if d.catalog.skills.is_empty() {
        return "No skills found. Add one as .forge/skills/<name>/SKILL.md (or in your config directory).".into();
    }
    let mut s = format!("{} skill(s):\n", d.catalog.skills.len());
    for k in &d.catalog.skills {
        let who = match (k.user_invocable, k.model_invocable) {
            (true, true) => "you and Forge",
            (true, false) => "you only",
            (false, true) => "Forge only",
            (false, false) => "hidden",
        };
        let _ = writeln!(
            s,
            "  {} - {} [{}; {}; ~{} tokens when loaded]",
            k.name,
            k.description,
            k.source,
            who,
            tokens_of(&k.body)
        );
    }
    s.trim_end().to_string()
}

fn agents(d: &Driver) -> String {
    let mut s = String::from("Subagents:\n");
    for a in &d.catalog.agents {
        let tools = a.tools.as_ref().map(|t| t.join(", ")).unwrap_or_else(|| "all tools".into());
        let _ = writeln!(s, "  {} ({:?}) - {} [{}]", a.name, a.source, a.description, tools);
    }
    s.push_str(
        "\nTo add one, ask Forge to create it, or write .forge/agents/<name>.md (a `---` header with name, \
         description and optional tools and model, then its instructions).",
    );
    s
}

fn hooks(d: &Driver) -> String {
    let h = d.engine.hooks();
    if h.disabled {
        return "Hooks are disabled (--bare, --safe-mode or disableAllHooks).".into();
    }
    if h.config.is_empty() {
        return "No hooks configured. Add them under \"hooks\" in a settings file.".into();
    }
    let mut s = String::new();
    for event in forge_hooks::HookEvent::ALL {
        let Some(matchers) = h.config.events.get(&event).filter(|m| !m.is_empty()) else { continue };
        let _ = writeln!(s, "{}:", event.as_str());
        for m in matchers {
            for cmd in &m.hooks {
                let _ = writeln!(
                    s,
                    "  [{}] {} (timeout {}s)",
                    m.pattern.as_deref().unwrap_or("*"),
                    cmd.command,
                    cmd.timeout.as_secs()
                );
            }
        }
    }
    s.trim_end().to_string()
}

fn memory(d: &Driver) -> String {
    let files = forge_config::load_memory(&d.info.cwd);
    if files.is_empty() {
        return "No memory files. Create FORGE.md (or AGENTS.md) in the project, or FORGE.md in your config directory."
            .into();
    }
    let mut s = String::from("Memory files, loaded in this order (later ones take precedence):\n");
    for f in &files {
        let _ = writeln!(s, "  {} ({:?}, ~{} tokens)", f.path.display(), f.kind, tokens_of(&f.content));
    }
    s.trim_end().to_string()
}

fn doctor(d: &Driver) -> Exec {
    let mut checks = crate::doctor::checks(&d.info.cwd);
    checks.push(crate::doctor::Check {
        ok: d.info.warnings.is_empty(),
        name: "session",
        detail: if d.info.warnings.is_empty() { "no warnings".into() } else { d.info.warnings.join("; ") },
    });
    if let Some(m) = &d.catalog.mcp {
        let failed = m.warnings();
        checks.push(crate::doctor::Check {
            ok: failed.is_empty(),
            name: "mcp",
            detail: if failed.is_empty() { format!("{} server(s) fine", m.servers.len()) } else { failed.join("; ") },
        });
    }
    let model = d.handle().model();
    let priced = forge_api::models::model_info(&model).is_some();
    checks.push(crate::doctor::Check {
        ok: priced,
        name: "model",
        detail: if priced {
            format!("{model} (known pricing)")
        } else {
            format!("{model}: unknown pricing; costs show as $0 and --max-budget-usd refuses it")
        },
    });
    let failed = checks.iter().any(|c| !c.ok);
    Exec::Local { text: crate::doctor::render(&checks), is_error: failed }
}

fn release_notes() -> String {
    include_str!("../../../../CHANGELOG.md").trim().to_string()
}

fn plugins(d: &Driver) -> String {
    if d.catalog.plugins.is_empty() {
        return "No plugins loaded. Load one with --plugin-dir <dir> or the pluginDirs setting.".into();
    }
    let mut s = String::from("Plugins:\n");
    for p in &d.catalog.plugins {
        let version = p.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        let _ = writeln!(s, "  {}{version} - {} ({})", p.name, p.description, p.dir.display());
    }
    s.trim_end().to_string()
}

fn mcp(d: &Driver) -> String {
    let Some(m) = &d.catalog.mcp else { return "No MCP servers configured.".into() };
    if m.servers.is_empty() {
        return "No MCP servers configured. Add one with `forge mcp add`.".into();
    }
    let mut s = String::from("MCP servers:\n");
    for e in &m.servers {
        let detail = match &e.status {
            forge_mcp::Status::Connected => format!("connected, {} tools, {} prompts", e.tools.len(), e.prompts.len()),
            forge_mcp::Status::Failed(why) => format!("failed: {why}"),
            forge_mcp::Status::Skipped(why) => why.clone(),
        };
        let _ = writeln!(
            s,
            "  {} [{}{}] {}",
            e.name,
            e.scope,
            if e.transport.is_empty() { String::new() } else { format!(", {}", e.transport) },
            detail
        );
    }
    s.trim_end().to_string()
}

fn tasks(d: &Driver, args: &str) -> Exec {
    let shells = &d.engine.tool_ctx().shells;
    if let Some(id) = args.strip_prefix("stop").map(str::trim).filter(|s| !s.is_empty()) {
        return match shells.get(id) {
            Some(sh) => {
                sh.kill();
                ok(format!("Stopped {id}."))
            }
            None => err(format!("No background task {id}.")),
        };
    }
    if !args.is_empty() {
        return err("Usage: /tasks [stop <id>]");
    }
    let list = shells.list();
    if list.is_empty() {
        return ok("No background tasks.");
    }
    let mut s = String::from("Background tasks:\n");
    for sh in list {
        let _ = writeln!(
            s,
            "  {} [{}, {}] {}",
            sh.id,
            sh.status().label(),
            duration(sh.started.elapsed()),
            sh.command.chars().take(100).collect::<String>()
        );
    }
    ok(s.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_and_durations() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(duration(std::time::Duration::from_secs(75)), "1m 15s");
        assert_eq!(duration(std::time::Duration::from_secs(7322)), "2h 02m");
    }
}
