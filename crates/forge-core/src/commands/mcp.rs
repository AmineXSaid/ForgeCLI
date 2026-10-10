//! `/mcp`: the MCP servers' status, and `reconnect`, `enable` or `disable`
//! for one server or `all`. The work is [`forge_mcp::McpManager::apply`],
//! which the stream-json `mcp_reconnect` and `mcp_toggle` requests share.

use std::fmt::Write as _;

use forge_mcp::{Outcome, ServerAction, Status};

use super::run::{err, ok};
use super::{Exec, Surface};
use crate::view::{Effect, SessionView, ViewState};

const USAGE: &str = "Usage: /mcp [reconnect|enable|disable <server|all>]";

/// `/mcp`, from the session view: it runs mid-turn too. The manager serializes each server's
/// changes, so tool calls in flight see a consistent state; what the session derives from the
/// servers (instructions, `system/init`) is refreshed by the driver afterwards.
pub(super) async fn run(v: &SessionView, args: &str) -> Exec {
    let d = v.state();
    let d = d.as_ref();
    if args.is_empty() {
        return ok(listing(d));
    }
    let words: Vec<&str> = args.split_whitespace().collect();
    let (Some(action), [_, target]) = (words.first().copied().and_then(ServerAction::parse), words.as_slice()) else {
        return err(USAGE);
    };
    let m = match d.mcp.clone() {
        Some(m) if !m.servers.is_empty() => m,
        _ => return err("No MCP servers configured. Add one with `forge mcp add`."),
    };
    let results = match m.apply(action, target).await {
        Ok(r) => r,
        Err(e) => return err(e),
    };
    v.record(Effect::RefreshMcp);
    let all = *target == "all";
    let mut lines = vec![];
    let mut failed = false;
    for r in &results {
        match r {
            Err(e) => {
                failed = true;
                lines.push(e.clone());
            }
            Ok(o) if all && o.unchanged => {}
            Ok(o) => {
                let (text, bad) = describe(d, action, o);
                failed |= bad;
                lines.push(text);
            }
        }
    }
    if lines.is_empty() {
        lines.push(
            match action {
                ServerAction::Reconnect => "No MCP servers to reconnect.",
                ServerAction::Enable => "Every MCP server is already enabled.",
                ServerAction::Disable => "Every MCP server is already disabled.",
            }
            .to_string(),
        );
    }
    Exec::Local { text: lines.join("\n"), is_error: failed }
}

/// One server's line, and whether it reports a failure.
fn describe(d: &ViewState, action: ServerAction, o: &Outcome) -> (String, bool) {
    let name = &o.server;
    let counts = format!("connected, {} tools, {} prompts", o.tools, o.prompts);
    let saved = match &o.saved {
        Some(file) => format!(" Saved in {}.", file.display()),
        None => " This session only: the server comes from --mcp-config or a plugin.".to_string(),
    };
    let mut text = match (action, &o.status, o.unchanged) {
        (ServerAction::Disable, _, true) => format!("{name} is already disabled."),
        (ServerAction::Disable, _, false) => format!("Disabled {name}: its tools and prompts are hidden.{saved}"),
        (ServerAction::Enable, Status::Failed(why), true) => {
            return (format!("{name} is enabled but failed: {why}. /mcp reconnect {name} tries again."), true)
        }
        (ServerAction::Enable, _, true) => format!("{name} is already enabled."),
        (ServerAction::Enable, Status::Failed(why), false) => {
            return (format!("Enabled {name}, but it failed to start: {why}.{saved}"), true)
        }
        (ServerAction::Enable, _, false) => format!("Enabled {name}: {counts}.{saved}"),
        (ServerAction::Reconnect, Status::Failed(why), _) => {
            return (format!("Could not reconnect {name}: {why}"), true)
        }
        (ServerAction::Reconnect, _, _) => format!("Reconnected {name}: {counts}."),
    };
    if !o.gone_tools.is_empty() {
        let _ = write!(text, " No longer offered: {}.", o.gone_tools.join(", "));
    }
    if !o.new_tools.is_empty() {
        let when = if d.can_switch && matches!(d.surface, Surface::Repl | Surface::Tui | Surface::Stream) {
            "after /reload-plugins"
        } else {
            "in a new session"
        };
        let _ = write!(text, " New tools join {when}: {}.", o.new_tools.join(", "));
    }
    if action == ServerAction::Enable && !o.unchanged {
        for layer in forge_mcp::config::disabled_in(&d.settings, name) {
            if layer.source == forge_config::SettingSource::User {
                let file =
                    layer.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "your user settings".into());
                let _ = write!(text, " It is still disabled in {file}, so new sessions start without it.");
            }
        }
    }
    (text, false)
}

/// Each server and how it is.
fn listing(d: &ViewState) -> String {
    let Some(m) = &d.mcp else { return "No MCP servers configured.".into() };
    if m.servers.is_empty() {
        return "No MCP servers configured. Add one with `forge mcp add`.".into();
    }
    let mut s = String::from("MCP servers:\n");
    for e in &m.servers {
        let detail = match e.status() {
            Status::Connected => format!("connected, {} tools, {} prompts", e.tools().len(), e.prompts().len()),
            Status::Failed(why) => format!("failed: {why}"),
            Status::Skipped(why) => why,
            Status::Pending => "reconnecting".into(),
        };
        let transport = if e.transport.is_empty() { String::new() } else { format!(", {}", e.transport) };
        let _ = writeln!(s, "  {} [{}{transport}] {detail}", e.name, e.scope);
    }
    s.push_str("\nManage them with /mcp reconnect, enable or disable <server|all>.");
    s
}
