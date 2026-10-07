//! `/import [codex|gemini|cursor] [--yes]`: MCP servers and instructions from
//! other coding agents. Without `--yes` it shows the plan; with it, applies it.

use std::fmt::Write as _;

use super::run::{err, ok};
use super::Exec;
use crate::driver::Driver;
use crate::import::{apply, describe_server, plan, Places, Source};

pub(super) fn run(d: &Driver, args: &str) -> Exec {
    let mut sources = vec![];
    let mut yes = false;
    for w in args.split_whitespace() {
        match w {
            "--yes" | "-y" => yes = true,
            w => match Source::parse(w) {
                Some(s) if !sources.contains(&s) => sources.push(s),
                Some(_) => {}
                None => return err(format!("Unknown source {w:?}. Usage: /import [codex|gemini|cursor] [--yes]")),
            },
        }
    }
    let all = sources.is_empty();
    if all {
        sources = Source::ALL.to_vec();
    }
    let (home, config) = (forge_config::home(), forge_config::config_dir());
    let at = Places { home: &home, project: &d.info.cwd, user_settings: &d.info.user_settings, config_dir: &config };
    let p = plan(&sources, &at);
    let names = sources.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ");
    let mut s = String::new();
    if p.is_empty() {
        let _ = write!(s, "Nothing to import from {names}.");
    } else if !yes {
        s.push_str("Found to import:\n");
        for x in &p.servers {
            let dest = if x.to == d.info.user_settings {
                "your user settings".to_string()
            } else {
                format!("{} (approve it with `forge mcp approve {}`)", x.to.display(), x.name)
            };
            let _ = writeln!(
                s,
                "  MCP server {} from {}: {} -> {dest}",
                x.name,
                x.source.name(),
                describe_server(&x.config)
            );
        }
        for i in &p.instructions {
            let _ = writeln!(
                s,
                "  {} ({} characters, from {}) -> {}",
                i.heading.trim_start_matches("## "),
                i.text.chars().count(),
                i.from.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", "),
                i.to.display()
            );
        }
        let again = if all {
            "/import --yes".to_string()
        } else {
            format!(
                "/import {} --yes",
                args.split_whitespace().filter(|w| !w.starts_with('-')).collect::<Vec<_>>().join(" ")
            )
        };
        let _ = write!(s, "Run {again} to apply.");
    } else {
        match apply(&p) {
            Ok(done) => {
                for line in done {
                    let _ = writeln!(s, "{line}.");
                }
                s.push_str("Imported servers and instructions take effect in your next session (or after /clear).");
                if p.servers.iter().any(|x| x.to != d.info.user_settings) {
                    s.push_str(" Project servers in .mcp.json need `forge mcp approve <name>` first.");
                }
            }
            Err(e) => return err(format!("Import failed: {e}")),
        }
    }
    if !p.skipped.is_empty() {
        let _ = write!(s, "\nSkipped:\n  {}", p.skipped.join("\n  "));
    }
    ok(s.trim_end())
}
