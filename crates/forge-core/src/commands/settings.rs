//! Commands that change how the session runs: `/model`, `/effort`, `/fast`,
//! `/config`, `/output-style`, `/autocompact` and `/sandbox`.
//!
//! Each applies at once. In the REPL and the TUI the change is also saved as
//! the default (the same settings file the reference writes); in `-p` and
//! stream-json it lasts for this session only, so a script never changes the
//! user's defaults by accident. `/config key=value` always saves: saving is
//! what it is for.

use std::fmt::Write as _;
use std::path::PathBuf;

use forge_api::models::{model_info, model_info_or_default, MODELS};
use forge_config::SettingSource;
use serde_json::{json, Value};

use super::run::{err, ok};
use super::{Exec, Surface};
use crate::driver::{Driver, SessionInfo};

/// A settings file a command writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
    Local,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "user" => Some(Scope::User),
            "project" => Some(Scope::Project),
            "local" => Some(Scope::Local),
            _ => None,
        }
    }

    pub fn source(self) -> SettingSource {
        match self {
            Scope::User => SettingSource::User,
            Scope::Project => SettingSource::Project,
            Scope::Local => SettingSource::Local,
        }
    }

    pub fn path(self, info: &SessionInfo) -> PathBuf {
        match self {
            Scope::User => info.user_settings.clone(),
            Scope::Project => info.cwd.join(".forge/settings.json"),
            Scope::Local => info.cwd.join(".forge/settings.local.json"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::User => "user settings",
            Scope::Project => "project settings",
            Scope::Local => "local project settings",
        }
    }
}

/// Write `keys` = `value` (`None` removes it) to `scope`'s file and mirror it
/// in memory. Returns a sentence saying where it went, and which layer, if
/// any, overrides it.
pub(super) fn save(d: &mut Driver, scope: Scope, keys: &[&str], value: Option<Value>) -> Result<String, String> {
    let path = scope.path(&d.info);
    let written = match &value {
        Some(v) => forge_config::write_setting(&path, keys, v.clone()),
        None => forge_config::remove_setting(&path, keys).map(|_| ()),
    };
    written.map_err(|e| format!("could not write {}: {e}", path.display()))?;
    d.info.settings.apply(scope.source(), &path, keys, value.clone());
    let mut note = match value {
        Some(_) => format!("Saved in {} ({}).", scope.label(), path.display()),
        None => format!("Removed from {} ({}).", scope.label(), path.display()),
    };
    if let Some(l) = d.info.settings.overridden_by(scope.source(), &format!("/{}", keys.join("/"))) {
        let at = l.path.as_ref().map(|p| format!(" ({})", p.display())).unwrap_or_default();
        let _ = write!(note, " {} also sets {} and takes precedence{at}.", l.source.as_str(), keys.join("."));
    }
    Ok(note)
}

/// Save on interactive surfaces; elsewhere say the change is for this session.
fn save_default(d: &mut Driver, scope: Scope, keys: &[&str], value: Option<Value>) -> String {
    if !matches!(d.surface, Surface::Repl | Surface::Tui) {
        return "(This session only.)".into();
    }
    save(d, scope, keys, value).unwrap_or_else(|e| format!("Not saved: {e}."))
}

fn join(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" ")
}

fn window_label(n: u64) -> String {
    if n >= 1_000_000 && n.is_multiple_of(1_000_000) {
        format!("{}M", n / 1_000_000)
    } else {
        format!("{}k", n / 1_000)
    }
}

const ALIASES: &[&str] = &["default", "opus", "sonnet", "haiku", "fable"];

fn model_list(current: &str) -> String {
    let info = model_info_or_default(current);
    let mut s = format!("Current model: {} ({current})\n\nModels:\n", info.display_name);
    for m in MODELS {
        let aliases: Vec<&str> = ALIASES.iter().copied().filter(|a| forge_api::resolve_model(a) == m.id).collect();
        let _ = writeln!(
            s,
            "{} {:<20} {:<10} {} context · ${}/${} per Mtok{}{}",
            if m.id == current { "*" } else { " " },
            m.id,
            m.display_name,
            window_label(m.context_window),
            m.input_price,
            m.output_price,
            if m.supports_fast_mode { " · fast mode" } else { "" },
            if aliases.is_empty() { String::new() } else { format!(" ({})", aliases.join(", ")) },
        );
    }
    s.push_str("\nSwitch with /model <alias or id>.");
    s
}

/// Switch the running model; returns notes about settings it affects.
fn apply_model(d: &mut Driver, id: &str) -> Result<Vec<String>, String> {
    let known = model_info(id);
    if known.is_none() && d.engine.cfg.max_budget_usd.is_some() && !d.engine.cfg.pricing.contains_key(id) {
        return Err(format!(
            "{id} has no known pricing, so --max-budget-usd can't be enforced. Add it under \"modelPricing\" in settings."
        ));
    }
    let h = d.engine.handle();
    h.set_model(id);
    d.prompt.set_model(id);
    d.rebuild_system();
    d.info.init.model = id.to_string();
    let rt = h.runtime();
    let mut notes = vec![];
    let info = model_info_or_default(id);
    if known.is_none() {
        notes.push(format!("Forge doesn't know {id}: its context window, limits and pricing are guesses."));
    }
    if let Some(e) = &rt.effort {
        if !info.effort_levels.contains(&e.as_str()) {
            notes.push(format!("Effort {e} isn't available on {}; it uses the model's default.", info.display_name));
        }
    }
    if rt.fast && !info.supports_fast_mode {
        notes.push(format!("Fast mode isn't available on {}, so it's paused.", info.display_name));
    }
    Ok(notes)
}

pub(super) fn model(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return ok(model_list(&d.engine.handle().model()));
    }
    let id = forge_api::resolve_model(args);
    let notes = match apply_model(d, &id) {
        Ok(n) => n,
        Err(e) => return err(e),
    };
    let saved = save_default(d, Scope::User, &["model"], Some(json!(args)));
    let name = model_info_or_default(&id).display_name;
    let name = if model_info(&id).is_some() { format!("{name} ({id})") } else { id.clone() };
    ok(join(&[format!("Set model to {name}."), notes.join(" "), saved]))
}

pub(super) fn effort(d: &mut Driver, args: &str) -> Exec {
    let h = d.engine.handle();
    let rt = h.runtime();
    let info = model_info_or_default(&rt.model);
    let levels = info.effort_levels;
    let default = info.default_effort.unwrap_or("none");
    match args {
        "" | "status" => {
            if levels.is_empty() {
                return ok(format!("{} doesn't support effort levels.", info.display_name));
            }
            let now = match &rt.effort {
                Some(e) => e.clone(),
                None => format!("auto ({}'s default: {default})", info.display_name),
            };
            ok(format!("Effort: {now}\nLevels for {}: {}, or auto.", info.display_name, levels.join(", ")))
        }
        _ if levels.is_empty() => err(format!("{} doesn't support effort levels.", info.display_name)),
        "auto" => {
            h.set_effort(None);
            let saved = save_default(d, Scope::User, &["effortLevel"], None);
            ok(join(&[format!("Effort set to auto ({}'s default: {default}).", info.display_name), saved]))
        }
        level if levels.contains(&level) => {
            h.set_effort(Some(level.to_string()));
            // `max` is for this session only, as in the reference.
            let saved = if level == "max" {
                "(max lasts for this session only.)".to_string()
            } else {
                save_default(d, Scope::User, &["effortLevel"], Some(json!(level)))
            };
            ok(join(&[format!("Set effort to {level}."), saved]))
        }
        other => err(format!("Unknown effort level {other:?}. Choose one of: {}, or auto.", levels.join(", "))),
    }
}

fn fast_models() -> String {
    MODELS.iter().filter(|m| m.supports_fast_mode).map(|m| m.display_name).collect::<Vec<_>>().join(", ")
}

pub(super) fn fast(d: &mut Driver, args: &str) -> Exec {
    let h = d.engine.handle();
    let rt = h.runtime();
    let info = model_info_or_default(&rt.model);
    let supported = info.supports_fast_mode && d.engine.provider_name() != "openai";
    let on = match args {
        "" => !rt.fast,
        "on" => true,
        "off" => false,
        "status" => {
            return ok(match (rt.fast, supported) {
                (true, true) => format!("Fast mode is on for {}.", info.display_name),
                (true, false) => format!("Fast mode is on but paused: {} doesn't offer it.", info.display_name),
                (false, _) => "Fast mode is off.".to_string(),
            })
        }
        _ => return err("Usage: /fast [on|off|status]"),
    };
    if on && !supported {
        return err(format!(
            "Fast mode isn't available for {}. Models with fast mode: {}.",
            info.display_name,
            fast_models()
        ));
    }
    h.set_fast(on);
    let saved = save_default(d, Scope::User, &["fastMode"], Some(json!(on)));
    let text = if on {
        format!("Fast mode on: faster output from {}, at a higher price per token.", info.display_name)
    } else {
        "Fast mode off.".to_string()
    };
    ok(join(&[text, saved]))
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Bool,
    Str,
    Enum(&'static [&'static str]),
    /// `auto` or a token count (`200k`).
    Window,
}

struct Key {
    name: &'static str,
    kind: Kind,
    /// Where `/config` saves it unless `--scope` says otherwise.
    scope: Scope,
    /// Takes effect in this session (otherwise from the next one).
    live: bool,
    help: &'static str,
}

/// `bypassPermissions` is left out: it takes `--dangerously-skip-permissions`.
const MODES: &[&str] = &["default", "acceptEdits", "plan", "dontAsk", "auto"];

const KEYS: &[Key] = &[
    Key { name: "model", kind: Kind::Str, scope: Scope::User, live: true, help: "Default model (alias or id)" },
    Key {
        name: "effortLevel",
        kind: Kind::Enum(&["low", "medium", "high", "xhigh", "auto"]),
        scope: Scope::User,
        live: true,
        help: "Reasoning effort, where the model supports it",
    },
    Key {
        name: "fastMode",
        kind: Kind::Bool,
        scope: Scope::User,
        live: true,
        help: "Fast mode, where the model offers it",
    },
    Key {
        name: "alwaysThinkingEnabled",
        kind: Kind::Bool,
        scope: Scope::User,
        live: true,
        help: "Extended thinking (false turns it off where the model allows)",
    },
    Key {
        name: "outputStyle",
        kind: Kind::Str,
        scope: Scope::Local,
        live: true,
        help: "Output style (see /output-style)",
    },
    Key {
        name: "autoCompactEnabled",
        kind: Kind::Bool,
        scope: Scope::User,
        live: true,
        help: "Summarize the conversation when the context is nearly full",
    },
    Key {
        name: "autoCompactWindow",
        kind: Kind::Window,
        scope: Scope::User,
        live: true,
        help: "Window compaction measures against: auto or 100k-1M tokens",
    },
    Key {
        name: "verification.enabled",
        kind: Kind::Bool,
        scope: Scope::User,
        live: true,
        help: "Remind the model to run the project's checks before finishing",
    },
    Key {
        name: "permissions.defaultMode",
        kind: Kind::Enum(MODES),
        scope: Scope::Local,
        live: true,
        help: "Permission mode sessions start in",
    },
    Key {
        name: "smallFastModel",
        kind: Kind::Str,
        scope: Scope::User,
        live: false,
        help: "Model for background work (titles, web summaries, checks)",
    },
];

fn config_help() -> String {
    let mut s = String::from(
        "Usage: /config [key=value ...] [--scope user|project|local]\n\
         key= (no value) removes the key, restoring the default.\n\nKeys:\n",
    );
    for k in KEYS {
        let kind = match k.kind {
            Kind::Bool => "true|false".to_string(),
            Kind::Str => "text".to_string(),
            Kind::Enum(v) => v.join("|"),
            Kind::Window => "auto|<tokens>".to_string(),
        };
        let _ = writeln!(
            s,
            "  {:<24} {:<34} {} [{}{}]",
            k.name,
            kind,
            k.help,
            k.scope.label(),
            if k.live { "" } else { "; next session" }
        );
    }
    s.trim_end().to_string()
}

fn pointer(name: &str) -> String {
    format!("/{}", name.replace('.', "/"))
}

fn config_show(d: &Driver) -> String {
    let st = &d.info.settings;
    let mut s = String::from("Settings (effective value, and the layer it comes from):\n");
    for k in KEYS {
        let p = pointer(k.name);
        let layer = st.layers.iter().rev().find(|l| l.value.pointer(&p).is_some_and(|v| !v.is_null()));
        let value = st.get(&p).map(|v| v.to_string()).unwrap_or_else(|| "(default)".into());
        let _ = writeln!(
            s,
            "  {:<24} {value}{}",
            k.name,
            layer.map(|l| format!("  [{}]", l.source.as_str())).unwrap_or_default()
        );
    }
    let files: Vec<String> = st
        .layers
        .iter()
        .filter_map(|l| l.path.as_ref().map(|p| format!("{} ({})", p.display(), l.source.as_str())))
        .collect();
    let _ = write!(
        s,
        "\nFiles: {}\nChange one with /config key=value; /config --help lists the keys. `forge config list` prints everything.",
        if files.is_empty() { "none".into() } else { files.join(", ") }
    );
    s
}

/// Parse `raw` for `key`; `Ok(None)` removes the key.
fn parse_value(k: &Key, raw: &str) -> Result<Option<Value>, String> {
    if raw.is_empty() {
        return Ok(None);
    }
    match k.kind {
        Kind::Bool => match raw {
            "true" | "on" | "yes" => Ok(Some(json!(true))),
            "false" | "off" | "no" => Ok(Some(json!(false))),
            _ => Err(format!("{}: expected true or false, got {raw:?}", k.name)),
        },
        Kind::Str => Ok(Some(json!(raw))),
        // Effort `auto` means "no level set": the model's default.
        Kind::Enum(v) if v.contains(&raw) => Ok((k.name != "effortLevel" || raw != "auto").then(|| json!(raw))),
        Kind::Enum(v) => Err(format!("{}: expected one of {}, got {raw:?}", k.name, v.join(", "))),
        Kind::Window => match crate::parse_autocompact(raw) {
            Ok(Some(n)) => Ok(Some(json!(n))),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("{}: {}", k.name, e.to_string().replace("--autocompact", "the window"))),
        },
    }
}

/// Apply a validated value to the running session; returns a note, if any.
fn apply_key(d: &mut Driver, name: &str, value: &Option<Value>) -> Result<Option<String>, String> {
    let h = d.engine.handle();
    let b = value.as_ref().and_then(Value::as_bool);
    let st = value.as_ref().and_then(Value::as_str);
    match name {
        "model" => {
            let id = forge_api::resolve_model(st.unwrap_or("default"));
            let notes = apply_model(d, &id)?;
            Ok((!notes.is_empty()).then(|| notes.join(" ")))
        }
        "effortLevel" => {
            h.set_effort(st.map(str::to_string));
            Ok(None)
        }
        "fastMode" => {
            let on = b.unwrap_or(false);
            h.set_fast(on);
            let supported = model_info_or_default(&h.model()).supports_fast_mode;
            Ok((on && !supported).then(|| "Fast mode is paused: the current model doesn't offer it.".to_string()))
        }
        "alwaysThinkingEnabled" => {
            h.set_max_thinking_tokens(if b == Some(false) { Some(0) } else { None });
            Ok(None)
        }
        "outputStyle" => {
            set_style(d, st.unwrap_or("default"))?;
            Ok(None)
        }
        "autoCompactEnabled" => {
            d.engine.cfg.auto_compact = b.unwrap_or(true);
            Ok(None)
        }
        "autoCompactWindow" => {
            d.engine.cfg.autocompact_window = value.as_ref().and_then(Value::as_u64);
            Ok(None)
        }
        "verification.enabled" => {
            d.engine.cfg.verify =
                if b == Some(false) { None } else { crate::verify_config(&d.info.settings, &d.info.cwd) };
            Ok(None)
        }
        "permissions.defaultMode" => {
            let mode = forge_permissions::PermissionMode::parse(st.unwrap_or("default"))
                .ok_or_else(|| "unknown permission mode".to_string())?;
            h.set_permission_mode(mode);
            Ok(None)
        }
        _ => Ok(Some(format!("{name} applies from the next session."))),
    }
}

pub(super) fn config(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return ok(config_show(d));
    }
    let mut words: Vec<&str> = args.split_whitespace().collect();
    if words.iter().any(|w| matches!(*w, "--help" | "-h" | "help")) {
        return ok(config_help());
    }
    let mut scope = None;
    if let Some(i) = words.iter().position(|w| *w == "--scope") {
        let Some(s) = words.get(i + 1).and_then(|s| Scope::parse(s)) else {
            return err("--scope takes user, project or local.");
        };
        scope = Some(s);
        words.drain(i..=i + 1);
    }
    // Validate everything before writing anything.
    let mut changes = vec![];
    for w in &words {
        let Some((name, raw)) = w.split_once('=') else {
            return err(format!("Expected key=value, got {w:?}. /config --help lists the keys."));
        };
        let Some(k) = KEYS.iter().find(|k| k.name == name) else {
            let names: Vec<&str> = KEYS.iter().map(|k| k.name).collect();
            return err(format!("Unknown setting {name:?}. Settable here: {}.", names.join(", ")));
        };
        let value = match parse_value(k, raw) {
            Ok(v) => v,
            Err(e) => return err(e),
        };
        if k.name == "outputStyle" {
            if let Some(name) = value.as_ref().and_then(Value::as_str) {
                if forge_agents::styles::find(&d.catalog.styles, name).is_none() {
                    return err(format!("No output style named {name:?}. /output-style lists them."));
                }
            }
        }
        changes.push((k, value));
    }
    let mut out = vec![];
    for (k, value) in changes {
        if let Err(e) = apply_key(d, k.name, &value) {
            return err(format!("{}: {e}", k.name));
        }
        let keys: Vec<&str> = k.name.split('.').collect();
        match save(d, scope.unwrap_or(k.scope), &keys, value.clone()) {
            Ok(note) => {
                let shown = value.map(|v| v.to_string()).unwrap_or_else(|| "(default)".into());
                out.push(format!("{} = {shown}. {note}", k.name));
            }
            Err(e) => return err(e),
        }
    }
    ok(out.join("\n"))
}

fn set_style(d: &mut Driver, name: &str) -> Result<(), String> {
    let style = forge_agents::styles::find(&d.catalog.styles, name)
        .ok_or_else(|| format!("No output style named {name:?}. /output-style lists them."))?
        .clone();
    d.prompt.output_style = Some(style.prompt.clone()).filter(|p| !p.is_empty());
    d.rebuild_system();
    d.info.init.output_style = style.name;
    Ok(())
}

pub(super) fn output_style(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        let mut s = String::from("Output styles:\n");
        for st in &d.catalog.styles {
            let _ = writeln!(
                s,
                "{} {} - {}",
                if st.name == d.info.init.output_style { "*" } else { " " },
                st.name,
                st.description
            );
        }
        s.push_str("\nSwitch with /output-style <name>.");
        return ok(s);
    }
    if let Err(e) = set_style(d, args) {
        return err(e);
    }
    let name = d.info.init.output_style.clone();
    let saved = save_default(d, Scope::Local, &["outputStyle"], Some(json!(name)));
    ok(join(&[format!("Output style set to {name}. It applies from your next message."), saved]))
}

pub(super) fn autocompact(d: &mut Driver, args: &str) -> Exec {
    let model = d.engine.handle().model();
    match args {
        "" | "status" => {
            let window = d.engine.context_window(&model);
            let source = if d.engine.cfg.autocompact_window.is_some() { "set" } else { "the model's window" };
            ok(if d.engine.cfg.auto_compact {
                format!(
                    "Auto-compact is on: Forge summarizes the conversation at about {} tokens of a {} window ({source}).",
                    super::run::thousands(d.engine.autocompact_at(&model)),
                    window_label(window),
                )
            } else {
                "Auto-compact is off. /autocompact on turns it back on.".into()
            })
        }
        "on" | "off" => {
            let on = args == "on";
            d.engine.cfg.auto_compact = on;
            let saved = save_default(d, Scope::User, &["autoCompactEnabled"], Some(json!(on)));
            ok(join(&[format!("Auto-compact {args}."), saved]))
        }
        v => match crate::parse_autocompact(v) {
            Ok(window) => {
                d.engine.cfg.autocompact_window = window;
                let saved = save_default(d, Scope::User, &["autoCompactWindow"], window.map(|n| json!(n)));
                let what = match window {
                    Some(n) => format!("Auto-compact now measures against a {} window.", window_label(n)),
                    None => "Auto-compact now measures against the model's window.".to_string(),
                };
                ok(join(&[what, saved]))
            }
            Err(_) => err("Usage: /autocompact [on|off|auto|<tokens, 100k-1M>]"),
        },
    }
}

pub(super) fn sandbox(d: &mut Driver, args: &str) -> Exec {
    let ctx = d.engine.tool_ctx().clone();
    let backend = forge_tools::sandbox::backend();
    if matches!(args, "" | "status") {
        return ok(match (ctx.sandbox_policy(), backend) {
            (Some(p), Some(b)) => format!(
                "Sandbox: {} (network {}), via {b:?}. Shell commands run confined.",
                p.mode.as_str(),
                if p.network { "on" } else { "off" }
            ),
            (Some(p), None) => format!("Sandbox: {} requested, but no sandbox is available here.", p.mode.as_str()),
            (None, Some(b)) => format!("Sandbox: off ({b:?} is available). /sandbox on confines shell commands."),
            (None, None) => "Sandbox: off. None is available: install bubblewrap (Linux).".into(),
        });
    }
    let wanted = if args == "on" { "workspace-write" } else { args };
    let Some(mode) = forge_tools::sandbox::SandboxMode::parse(wanted) else {
        return err("Usage: /sandbox [on|off|read-only|workspace-write|status]");
    };
    if mode.is_some() && backend.is_none() {
        return err(if cfg!(target_os = "linux") {
            "No sandbox is available here: install bubblewrap (bwrap) to confine shell commands."
        } else {
            "No sandbox is available on this system."
        });
    }
    let st = &d.info.settings;
    ctx.set_sandbox(mode.map(|mode| forge_tools::sandbox::SandboxPolicy {
        mode,
        network: st.bool("/sandbox/network").unwrap_or(false),
        writable_roots: vec![],
        extra_writable: st.strings("/sandbox/writableRoots").into_iter().map(PathBuf::from).collect(),
    }));
    let name = mode.map(|m| m.as_str()).unwrap_or("off");
    let saved = save_default(d, Scope::Local, &["sandbox", "mode"], Some(json!(name)));
    ok(join(&[
        match mode {
            Some(_) => format!("Sandbox {name}: shell commands run confined."),
            None => "Sandbox off: shell commands run unconfined (and ask for approval as usual).".into(),
        },
        saved,
    ]))
}
