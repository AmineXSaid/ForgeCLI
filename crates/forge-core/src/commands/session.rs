//! Commands about this session: `/rename`, `/export`, `/diff`, `/context`,
//! `/debug`, `/permissions` and `/add-dir`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use forge_types::{ContentBlock, Message, MessageContent, MessagesRequest, Role, SystemBlock};
use serde_json::{json, Value};

use super::run::{err, ok, thousands, tokens_of};
use super::settings::{save, Scope};
use super::Exec;
use crate::driver::Driver;

/// The model for side requests: `smallFastModel`, else the default small
/// model; an OpenAI-compatible endpoint uses the session's model.
fn small_model(d: &Driver) -> String {
    if d.engine.provider_name() == "openai" {
        return d.engine.handle().model();
    }
    d.info
        .settings
        .str("/smallFastModel")
        .map(forge_api::resolve_model)
        .unwrap_or_else(|| forge_api::models::SMALL_FAST_MODEL.to_string())
}

/// A one-off request with no tools, outside the conversation.
pub(crate) async fn side_request(d: &Driver, system: &str, user: String, max_tokens: u32) -> Result<String, String> {
    let req = MessagesRequest {
        model: small_model(d),
        max_tokens,
        messages: vec![Message::user(vec![ContentBlock::text(user)])],
        system: vec![SystemBlock::text(system)],
        tools: vec![],
        tool_choice: None,
        thinking: None,
        temperature: None,
        metadata: None,
        output_config: None,
        speed: None,
        stream: true,
        betas: vec![],
    };
    let provider = d.engine.provider();
    let msg = forge_api::complete(provider.as_ref(), req, &tokio_util::sync::CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    Ok(msg.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("").trim().to_string())
}

fn is_reminder(text: &str) -> bool {
    text.trim_start().starts_with("<system-reminder>")
}

/// One line per tool call: `Name(first argument)`.
fn tool_call_line(name: &str, input: &Value) -> String {
    let arg = input
        .as_object()
        .and_then(|o| o.values().find_map(Value::as_str))
        .map(|s| {
            let one = s.lines().next().unwrap_or_default();
            if one.chars().count() > 80 {
                format!("{}…", one.chars().take(80).collect::<String>())
            } else {
                one.to_string()
            }
        })
        .unwrap_or_default();
    format!("⏺ {name}({arg})")
}

/// The conversation as plain text, as `/export` writes it.
pub fn render_conversation(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        for b in &m.content {
            match (m.role, b) {
                (Role::User, ContentBlock::Text { text, .. }) if !is_reminder(text) => {
                    let mut lines = text.trim().lines();
                    let _ = writeln!(out, "> {}", lines.next().unwrap_or_default());
                    for l in lines {
                        let _ = writeln!(out, "  {l}");
                    }
                    out.push('\n');
                }
                (Role::User, ContentBlock::ToolResult { content, is_error, .. }) => {
                    let text = content.to_text();
                    let n = text.lines().count();
                    let first = text.lines().next().unwrap_or_default().chars().take(100).collect::<String>();
                    let tag = if *is_error == Some(true) { "error: " } else { "" };
                    let more = if n > 1 { format!(" (+{} lines)", n - 1) } else { String::new() };
                    let _ = writeln!(out, "  ⎿  {tag}{first}{more}\n");
                }
                (Role::Assistant, ContentBlock::Text { text, .. }) if !text.trim().is_empty() => {
                    let _ = writeln!(out, "{}\n", text.trim());
                }
                (Role::Assistant, ContentBlock::ToolUse { name, input, .. }) => {
                    let _ = writeln!(out, "{}", tool_call_line(name, input));
                }
                _ => {}
            }
        }
    }
    out.trim_end().to_string()
}

/// A session name: no control characters, single spaces, at most 200 characters.
pub fn clean_title(raw: &str) -> String {
    let s: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(|c: char| c == '"' || c == '\'' || c == '`').trim().trim_end_matches('.').trim();
    s.chars().take(200).collect()
}

const TITLE_PROMPT: &str = "You name coding sessions. Reply with only a short title (three to seven words) for \
                            the conversation you are given: sentence case, no quotes, no closing punctuation.";

pub(super) async fn rename(d: &mut Driver, args: &str) -> Exec {
    let name = if args.is_empty() {
        let text = render_conversation(&d.engine.state.messages);
        if text.is_empty() {
            return err("Nothing to name yet. Send a message first, or give a name: /rename <name>");
        }
        let tail: String = text.chars().rev().take(6000).collect::<Vec<_>>().into_iter().rev().collect();
        match side_request(d, TITLE_PROMPT, tail, 60).await {
            Ok(t) => t,
            Err(e) => return err(format!("Could not generate a name: {e}. Give one instead: /rename <name>")),
        }
    } else {
        args.to_string()
    };
    let name = clean_title(&name);
    if name.is_empty() {
        return err("The name is empty. Usage: /rename [name]");
    }
    d.engine.transcript().set_title(&name);
    ok(format!("Session renamed to: {name}"))
}

pub(super) fn export(d: &Driver, args: &str) -> Exec {
    let body = render_conversation(&d.engine.state.messages);
    if body.is_empty() {
        return err("Nothing to export yet.");
    }
    let title = d.engine.transcript().title().map(|t| format!(" ({t})")).unwrap_or_default();
    let text = format!(
        "ForgeCLI conversation {}{title}\nExported {}\n\n{body}\n",
        d.info.session_id,
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    );
    if args.is_empty() {
        return ok(text.trim_end());
    }
    let path = forge_tools::expand_path(args, &d.info.cwd);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, text) {
        Ok(()) => ok(format!("Conversation exported to: {}", path.display())),
        Err(e) => err(format!("Could not write {}: {e}", path.display())),
    }
}

/// A prompt's first line, shortened, for listings.
fn excerpt(text: &str) -> String {
    let one = text.trim().lines().next().unwrap_or_default();
    if one.chars().count() > 60 {
        format!("{}…", one.chars().take(60).collect::<String>())
    } else {
        one.to_string()
    }
}

fn rel(p: &Path, cwd: &Path) -> String {
    p.strip_prefix(cwd).unwrap_or(p).display().to_string()
}

pub(super) fn diff(d: &Driver) -> Exec {
    let cwd = &d.info.cwd;
    let history = d.engine.history();
    let mut out = String::new();
    match forge_git::uncommitted(cwd) {
        Some((diff, untracked)) => {
            if diff.is_empty() && untracked.is_empty() {
                out.push_str("No uncommitted changes.\n");
            } else {
                if !diff.is_empty() {
                    let _ = writeln!(out, "Uncommitted changes (git diff HEAD):\n{diff}");
                }
                if !untracked.is_empty() {
                    let _ = writeln!(out, "\nUntracked files: {}", untracked.join(", "));
                }
            }
        }
        None => {
            // No git: diff the files Forge changed against how they were before.
            for (path, before) in history.originals() {
                let before = before.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                let after = std::fs::read_to_string(&path).unwrap_or_default();
                if before == after {
                    continue;
                }
                let name = rel(&path, cwd);
                let udiff = similar::TextDiff::from_lines(&before, &after)
                    .unified_diff()
                    .header(&format!("a/{name}"), &format!("b/{name}"))
                    .to_string();
                out.push_str(&udiff);
            }
            if out.is_empty() {
                out.push_str("Not a git repository, and Forge hasn't changed any files in this session.\n");
            }
        }
    }
    let turns = history.turns();
    if !turns.is_empty() {
        out.push_str("\nFiles Forge changed, by prompt:\n");
        let st = &d.engine.state;
        for (i, (turn, files)) in turns.iter().enumerate() {
            let prompt = st
                .uuids
                .iter()
                .position(|u| u == turn)
                .and_then(|j| st.messages.get(j))
                .map(|m| {
                    m.content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .find(|t| !is_reminder(t))
                        .map(excerpt)
                        .unwrap_or_default()
                })
                .unwrap_or_else(|| "(an earlier prompt)".into());
            let names: Vec<String> = files.iter().map(|f| rel(f, cwd)).collect();
            let _ = writeln!(out, "  {}. \"{prompt}\": {}", i + 1, names.join(", "));
        }
    }
    ok(out.trim_end())
}

fn pct(n: u64, of: u64) -> String {
    if of == 0 {
        return "-".into();
    }
    format!("{:.1}%", n as f64 * 100.0 / of as f64)
}

pub(super) fn context(d: &Driver, args: &str) -> Exec {
    let all = match args {
        "" => false,
        "all" => true,
        _ => return err("Usage: /context [all]"),
    };
    let model = d.engine.handle().model();
    let window = d.engine.context_window(&model);
    let est = |s: &str| tokens_of(s) as u64;
    let system: u64 = d.engine.system().iter().map(|b| est(&b.text)).sum();
    let specs = d.engine.tools().specs();
    let size = |s: &forge_types::ToolSpec| est(&serde_json::to_string(s).unwrap_or_default());
    let (mut builtin, mut mcp, mut skills) = (0u64, 0u64, 0u64);
    for s in &specs {
        match s.name.as_str() {
            n if n.starts_with("mcp__") => mcp += size(s),
            "Skill" => skills += size(s),
            _ => builtin += size(s),
        }
    }
    let memory_files = forge_config::load_memory(&d.info.cwd);
    let memory: u64 = memory_files.iter().map(|f| est(&f.content)).sum();
    let messages: u64 =
        d.engine.state.messages.iter().map(|m| est(&serde_json::to_string(m).unwrap_or_default())).sum();
    // Memory files ride in the first prompt: part of the messages once one is sent.
    let pending_memory = if d.engine.state.messages.is_empty() { memory } else { 0 };
    let total = system + builtin + mcp + skills + messages + pending_memory;
    let measured = d.engine.state.context_tokens;

    let mut s = format!(
        "Context: about {} of {} tokens ({}) for {model}",
        thousands(total),
        thousands(window),
        pct(total, window)
    );
    if measured > 0 {
        let _ = write!(s, "; the last request measured {}", thousands(measured));
    }
    s.push_str("\n\n");
    let mut row = |name: &str, n: u64| {
        let _ = writeln!(s, "  {name:<20} {:>10}  {:>6}", thousands(n), pct(n, window));
    };
    row("System prompt", system);
    row("Built-in tools", builtin);
    row("MCP tools", mcp);
    row("Skills (listing)", skills);
    if pending_memory > 0 {
        row("Memory files", memory);
    }
    row("Messages", messages);
    if memory > 0 && pending_memory == 0 {
        row("  of which memory", memory);
    }
    row("Free", window.saturating_sub(total));
    if d.engine.cfg.auto_compact {
        let _ = writeln!(s, "\nAuto-compact runs at about {} tokens.", thousands(d.engine.autocompact_at(&model)));
    }
    if all {
        s.push_str("\nTools:\n");
        let mut sorted: Vec<_> = specs.iter().map(|t| (t.name.clone(), size(t))).collect();
        sorted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (name, n) in sorted {
            let _ = writeln!(s, "  {name:<40} {:>8}", thousands(n));
        }
        if !memory_files.is_empty() {
            s.push_str("\nMemory files:\n");
            for f in &memory_files {
                let _ = writeln!(s, "  {:<40} {:>8}", f.path.display(), thousands(est(&f.content)));
            }
        }
        if !d.catalog.skills.is_empty() {
            s.push_str("\nSkills (only names and descriptions are loaded until one is used):\n");
            for sk in &d.catalog.skills {
                let _ = writeln!(s, "  {:<40} {:>8}", sk.name, thousands(est(&sk.description)));
            }
        }
    }
    let mut tips = vec![];
    if mcp * 5 > total && mcp > 10_000 {
        tips.push("MCP tools take a large share; turn off servers you don't need for this task.".to_string());
    }
    if let Some(f) = memory_files.iter().find(|f| est(&f.content) > 5_000) {
        tips.push(format!(
            "{} is long (over 5k tokens); trimming it saves context on every request.",
            f.path.display()
        ));
    }
    if total * 10 > window * 6 {
        tips.push("Over 60% full: /compact now keeps what matters before automatic compaction does.".to_string());
    }
    if !tips.is_empty() {
        let _ = write!(s, "\nSuggestions:\n  - {}", tips.join("\n  - "));
    }
    if !all {
        s.push_str("\n/context all lists each tool, memory file and skill.");
    }
    ok(s.trim_end())
}

const DEBUG_PROMPT: &str = "Debug logging is on for this session and writes to {log}. The user reports this \
problem:\n\n{issue}\n\nRead the end of that log (it can be long: start with the last few hundred lines, and search \
it for errors and warnings), together with the conversation so far. Explain what went wrong and why, quoting the log \
lines that show it, and suggest a fix or a workaround. If the log doesn't cover the problem, say what to reproduce so \
that it does.";

pub(super) fn debug(d: &mut Driver, args: &str) -> Exec {
    let path = crate::debug::log_path(&d.info.session_id);
    let log = match crate::debug::active() {
        Some(to) => to,
        None => {
            if let Err(e) = crate::debug::enable(&path) {
                return err(format!("Could not turn on debug logging: {e}"));
            }
            path.display().to_string()
        }
    };
    if let Some(dir) = path.parent() {
        d.engine.handle().permissions.write().unwrap().add_read_dir(dir);
    }
    if args.is_empty() {
        return ok(format!(
            "Debug logging is on, writing to {log}. Reproduce the problem, then run /debug <what went wrong> and \
             Forge will read the log."
        ));
    }
    Exec::Submit(MessageContent::Text(DEBUG_PROMPT.replace("{log}", &log).replace("{issue}", args)))
}

const BEHAVIORS: [&str; 3] = ["allow", "ask", "deny"];

/// The settings layer a rule comes from, by its normalized text.
fn rule_source(d: &Driver, behavior: &str, rule: &str) -> String {
    d.info
        .settings
        .layers
        .iter()
        .rev()
        .find(|l| {
            l.value
                .pointer(&format!("/permissions/{behavior}"))
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().filter_map(Value::as_str).any(|s| normalize_rule(s).as_deref() == Some(rule)))
        })
        .map(|l| l.source.as_str().to_string())
        .unwrap_or_else(|| "session".into())
}

fn normalize_rule(s: &str) -> Option<String> {
    forge_permissions::Rule::parse(s.trim()).ok().map(|r| r.to_string())
}

fn permissions_list(d: &Driver) -> String {
    let h = d.engine.handle();
    let p = h.permissions.read().unwrap();
    let mut s = format!("Permission mode: {}\n", p.mode.as_str());
    for behavior in BEHAVIORS {
        let rules = match behavior {
            "allow" => &p.rules.allow,
            "ask" => &p.rules.ask,
            _ => &p.rules.deny,
        };
        let title = format!("{}{}", behavior[..1].to_uppercase(), &behavior[1..]);
        if rules.is_empty() {
            let _ = writeln!(s, "\n{title}: none");
            continue;
        }
        let _ = writeln!(s, "\n{title}:");
        for r in rules {
            let text = r.to_string();
            let _ = writeln!(s, "  {text}  [{}]", rule_source(d, behavior, &text));
        }
    }
    let dirs = d.engine.tool_ctx().working_dirs.read().unwrap().clone();
    let _ = writeln!(s, "\nWorking directories:");
    for dir in dirs {
        let _ = writeln!(s, "  {}", dir.display());
    }
    s.push_str(
        "\nAdd a rule: /permissions add <allow|ask|deny> <rule> [--scope local|project|user|session]\n\
         Remove one: /permissions remove <rule>",
    );
    s
}

/// Split a trailing `--scope <name>` off `args`.
fn take_scope(args: &str) -> Result<(String, Option<String>), String> {
    match args.rsplit_once("--scope") {
        Some((rest, scope)) => {
            let scope = scope.trim();
            if scope.is_empty() || scope.contains(char::is_whitespace) {
                return Err("--scope takes one of local, project, user or session.".into());
            }
            Ok((rest.trim().to_string(), Some(scope.to_string())))
        }
        None => Ok((args.trim().to_string(), None)),
    }
}

fn rules_in(d: &Driver, scope: Scope, behavior: &str) -> Vec<Value> {
    let path = scope.path(&d.info);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.pointer(&format!("/permissions/{behavior}")).and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

pub(super) fn permissions(d: &mut Driver, args: &str) -> Exec {
    let (verb, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
    match verb {
        "" | "list" => ok(permissions_list(d)),
        "add" => {
            let (behavior, rest) = rest.trim().split_once(char::is_whitespace).unwrap_or((rest.trim(), ""));
            if !BEHAVIORS.contains(&behavior) {
                return err("Usage: /permissions add <allow|ask|deny> <rule> [--scope local|project|user|session]");
            }
            let (rule, scope) = match take_scope(rest) {
                Ok(x) => x,
                Err(e) => return err(e),
            };
            let parsed = match forge_permissions::Rule::parse(&rule) {
                Ok(r) => r,
                Err(e) => return err(format!("Invalid rule {rule:?}: {}", e.why)),
            };
            let text = parsed.to_string();
            let b = match behavior {
                "allow" => forge_permissions::Behavior::Allow,
                "ask" => forge_permissions::Behavior::Ask,
                _ => forge_permissions::Behavior::Deny,
            };
            let scope = match scope.as_deref() {
                None | Some("local") => Some(Scope::Local),
                Some("session") => None,
                Some(s) => match Scope::parse(s) {
                    Some(sc) => Some(sc),
                    None => return err("--scope takes one of local, project, user or session."),
                },
            };
            d.engine.handle().permissions.write().unwrap().rules.add(b, parsed);
            let saved = match scope {
                None => "(This session only.)".to_string(),
                Some(sc) => {
                    let mut list = rules_in(d, sc, behavior);
                    if !list.iter().any(|v| v.as_str().and_then(normalize_rule).as_deref() == Some(text.as_str())) {
                        list.push(json!(text));
                    }
                    match save(d, sc, &["permissions", behavior], Some(Value::Array(list))) {
                        Ok(note) => note,
                        Err(e) => return err(e),
                    }
                }
            };
            ok(format!("Added {behavior} rule {text}. {saved}"))
        }
        "remove" | "rm" => {
            let rule = rest.trim();
            let Some(text) = normalize_rule(rule) else {
                return err("Usage: /permissions remove <rule>");
            };
            let mut removed = vec![];
            {
                let h = d.engine.handle();
                let mut guard = h.permissions.write().unwrap();
                let rules = &mut guard.rules;
                for (name, list) in [("allow", &mut rules.allow), ("ask", &mut rules.ask), ("deny", &mut rules.deny)] {
                    let before = list.len();
                    list.retain(|r| r.to_string() != text);
                    if list.len() != before {
                        removed.push(format!("{name} (session)"));
                    }
                }
            }
            for sc in [Scope::User, Scope::Project, Scope::Local] {
                for behavior in BEHAVIORS {
                    let list = rules_in(d, sc, behavior);
                    let kept: Vec<Value> = list
                        .iter()
                        .filter(|v| v.as_str().and_then(normalize_rule).as_deref() != Some(text.as_str()))
                        .cloned()
                        .collect();
                    if kept.len() != list.len() {
                        if let Err(e) = save(d, sc, &["permissions", behavior], Some(Value::Array(kept))) {
                            return err(e);
                        }
                        removed.push(format!("{behavior} ({})", sc.path(&d.info).display()));
                    }
                }
            }
            if removed.is_empty() {
                return err(format!("No rule {text} is set. /permissions lists the rules."));
            }
            ok(format!("Removed {text} from: {}.", removed.join(", ")))
        }
        _ => err("Usage: /permissions [list | add <allow|ask|deny> <rule> [--scope ...] | remove <rule>]"),
    }
}

pub(super) fn add_dir(d: &mut Driver, args: &str) -> Exec {
    let (raw, save_it) = match args.strip_suffix("--save") {
        Some(r) => (r.trim(), true),
        None => (args, false),
    };
    if raw.is_empty() {
        return err("Usage: /add-dir <path> [--save]");
    }
    let path = forge_tools::expand_path(raw, &d.info.cwd);
    let dir: PathBuf = match path.canonicalize() {
        Ok(p) if p.is_dir() => p,
        Ok(_) => return err(format!("{} is not a directory.", path.display())),
        Err(e) => return err(format!("{}: {e}", path.display())),
    };
    let ctx = d.engine.tool_ctx().clone();
    if ctx.working_dirs.read().unwrap().iter().any(|w| dir.starts_with(w)) {
        return ok(format!("{} is already inside a working directory.", dir.display()));
    }
    ctx.working_dirs.write().unwrap().push(dir.clone());
    d.engine.handle().permissions.write().unwrap().add_directory(&dir);
    d.prompt.env.additional_dirs.push(dir.clone());
    let dirs = d.prompt.env.additional_dirs.clone();
    d.engine.transcript().append_meta(json!({"additionalDirectories": dirs}));
    d.engine.remind(format!(
        "The user added a working directory: {}. You can read and edit files there too.",
        dir.display()
    ));
    let saved = if save_it {
        let mut list: Vec<Value> = d
            .info
            .settings
            .layers
            .iter()
            .find(|l| l.source == forge_config::SettingSource::Local)
            .and_then(|l| l.value.pointer("/permissions/additionalDirectories").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        let entry = json!(dir.display().to_string());
        if !list.contains(&entry) {
            list.push(entry);
        }
        save(d, Scope::Local, &["permissions", "additionalDirectories"], Some(Value::Array(list)))
            .unwrap_or_else(|e| format!("Not saved: {e}."))
    } else {
        "(This session only; add --save to keep it.)".into()
    };
    ok(format!("Added working directory {}. {saved}", dir.display()))
}

pub(super) fn goal(d: &mut Driver, args: &str) -> Exec {
    use crate::goal::{is_clear_word, Goal, Status, MAX_CONDITION};
    if args.is_empty() {
        return ok(match &d.goal {
            None => "No goal set. Set one with /goal <condition>.".to_string(),
            Some(g) => {
                let spent = d.engine.state.total_cost_usd - g.cost_at_start;
                let mut s = format!("Goal: {}\nStatus: {}", g.condition, g.status.as_str());
                if let Some(p) = &g.paused {
                    let _ = write!(s, " (paused: {p})");
                }
                let _ = write!(
                    s,
                    "\nChecked {} time(s) · {} · ${spent:.4} spent",
                    g.checks,
                    super::run::duration(g.started.elapsed())
                );
                match (&g.status, &g.last_reason) {
                    (Status::Failed(r), _) | (_, Some(r)) if !r.is_empty() => {
                        let _ = write!(s, "\nLast check: {r}");
                    }
                    _ => {}
                }
                s
            }
        });
    }
    if is_clear_word(args) {
        return match d.goal.take() {
            Some(mut g) if g.is_active() => {
                let c = g.condition.clone();
                g.status = Status::Cleared;
                d.goal = Some(g);
                d.goal_changed();
                d.goal = None;
                ok(format!("Goal cleared: {c}"))
            }
            _ => ok("No goal set."),
        };
    }
    if d.info.settings.bool("/disableAllHooks") == Some(true) {
        return err("/goal is unavailable while disableAllHooks is set: goals run as a check at the end of each turn.");
    }
    if args.chars().count() > MAX_CONDITION {
        return err(format!(
            "The goal is too long ({} characters; the limit is {MAX_CONDITION}).",
            args.chars().count()
        ));
    }
    d.goal = Some(Goal::new(args, d.engine.state.total_cost_usd));
    d.goal_changed();
    Exec::Submit(MessageContent::Text(args.to_string()))
}

pub(super) async fn btw(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return match d.side_questions.last() {
            Some((q, a)) => ok(format!("/btw {q}\n\n{a}")),
            None => ok("No side questions yet. Ask one with /btw <question>: Forge answers from the conversation, without tools, and leaves the conversation as it was."),
        };
    }
    let earlier = d.side_questions.clone();
    match d.engine.side_question(args, &earlier).await {
        Ok(answer) => {
            d.side_questions.push((args.to_string(), answer.clone()));
            let extra = d.side_questions.len().saturating_sub(crate::driver::MAX_SIDE_QUESTIONS);
            d.side_questions.drain(..extra);
            ok(answer)
        }
        Err(e) => err(format!("Could not answer: {e}")),
    }
}

const RECAP_PROMPT: &str = "You summarize coding sessions. Given a conversation between a user and a coding agent, \
reply with one line (at most 30 words): what was asked, what has been done, and what is still open. No preamble.";

pub(super) async fn recap(d: &Driver) -> Exec {
    let text = crate::goal::evaluator_transcript(&d.engine.state.messages);
    if text.is_empty() {
        return err("Nothing to recap yet.");
    }
    match side_request(d, RECAP_PROMPT, text, 120).await {
        Ok(line) => ok(line.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")),
        Err(e) => err(format!("Could not summarize: {e}")),
    }
}

pub(super) fn plan(d: &mut Driver, args: &str) -> Exec {
    d.engine.handle().set_permission_mode(forge_permissions::PermissionMode::Plan);
    d.info.init.permission_mode = "plan".into();
    if args.is_empty() {
        return ok("Plan mode on: Forge will look around and propose a plan before changing anything. Approving the plan ends plan mode.");
    }
    Exec::Submit(MessageContent::Text(args.to_string()))
}
