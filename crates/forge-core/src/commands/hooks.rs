//! `/hooks`: list the configured hooks, numbered per event with the settings
//! layer each comes from, and `add` or `remove` one:
//!
//! - `/hooks add <Event> <matcher> <command> [--scope local|project|user] [--timeout <s>]`
//!   (`''` or `*` as the matcher matches everything; words after the matcher
//!   are the command, so it needs no quotes);
//! - `/hooks remove <Event> <n>` (`n` as `/hooks` numbers it).
//!
//! Changes are written to the settings file of that scope, and the session
//! reloads so they apply at once. The TUI's hooks screen runs these forms.

use std::fmt::Write as _;

use forge_config::SettingSource;
use forge_hooks::HookEvent;
use serde_json::{json, Value};

use super::run::{err, ok};
use super::screens::{Field, Form, Row, RowAction, Screen, Span, Tone};
use super::settings::{save, Scope};
use super::Exec;
use crate::driver::{Driver, Switch};

const USAGE: &str =
    "Usage: /hooks [add <Event> <matcher> <command> [--scope local|project|user] [--timeout <s>] | remove <Event> <n>]";

/// One configured hook command.
struct Entry {
    matcher: Option<String>,
    command: String,
    timeout: u64,
    /// `local`, `project`, `user`, `flag`, `managed` or `plugin`.
    source: &'static str,
    /// Where it sits in its layer's `hooks.<Event>` array: (matcher, command). `None` for plugins.
    at: Option<(SettingSource, usize, usize)>,
}

fn source_name(s: SettingSource) -> &'static str {
    match s {
        SettingSource::User => "user",
        SettingSource::Project => "project",
        SettingSource::Local => "local",
        SettingSource::Flag => "flag",
        SettingSource::Managed => "managed",
    }
}

/// The hooks of `event`, from each settings layer in order, then the plugins' (in effect but in no settings file).
fn entries(d: &Driver, event: HookEvent) -> Vec<Entry> {
    let mut out = vec![];
    for layer in &d.info.settings.layers {
        let Some(list) = layer.value.pointer(&format!("/hooks/{}", event.as_str())).and_then(Value::as_array) else {
            continue;
        };
        for (mi, m) in list.iter().enumerate() {
            let matcher = m.get("matcher").and_then(Value::as_str).map(str::to_string);
            for (hi, h) in m.get("hooks").and_then(Value::as_array).into_iter().flatten().enumerate() {
                let Some(command) = h.get("command").and_then(Value::as_str) else { continue };
                let timeout = h
                    .get("timeout")
                    .and_then(Value::as_f64)
                    .map(|t| t as u64)
                    .unwrap_or(forge_hooks::DEFAULT_TIMEOUT.as_secs());
                out.push(Entry {
                    matcher: matcher.clone(),
                    command: command.to_string(),
                    timeout,
                    source: source_name(layer.source),
                    at: Some((layer.source, mi, hi)),
                });
            }
        }
    }
    // Hooks in effect that no settings layer has come from plugins.
    let cfg = &d.engine.hooks().config;
    for m in cfg.events.get(&event).into_iter().flatten() {
        for h in &m.hooks {
            if !out.iter().any(|e| e.command == h.command && e.matcher == m.pattern) {
                out.push(Entry {
                    matcher: m.pattern.clone(),
                    command: h.command.clone(),
                    timeout: h.timeout.as_secs(),
                    source: "plugin",
                    at: None,
                });
            }
        }
    }
    out
}

fn matcher_label(m: &Option<String>) -> &str {
    m.as_deref().filter(|p| !p.is_empty()).unwrap_or("*")
}

fn disabled(d: &Driver) -> bool {
    d.engine.hooks().disabled
}

/// `/hooks` with no arguments: every event that has hooks, numbered.
fn listing(d: &Driver) -> String {
    if disabled(d) {
        return "Hooks are disabled (--bare, --safe-mode or disableAllHooks).".into();
    }
    let mut s = String::new();
    for event in HookEvent::ALL {
        let list = entries(d, event);
        if list.is_empty() {
            continue;
        }
        let _ = writeln!(s, "{}:", event.as_str());
        for (i, e) in list.iter().enumerate() {
            let _ = writeln!(
                s,
                "  {}. [{}] {} (timeout {}s, {})",
                i + 1,
                matcher_label(&e.matcher),
                e.command,
                e.timeout,
                e.source
            );
        }
    }
    if s.is_empty() {
        return "No hooks configured. Add one with /hooks add <Event> <matcher> <command>, or under \"hooks\" in a \
                settings file."
            .into();
    }
    s.push_str("\nAdd one with /hooks add <Event> <matcher> <command> [--scope ..]; remove one with /hooks remove <Event> <n>.");
    s
}

fn event_of(name: &str) -> Result<HookEvent, String> {
    HookEvent::parse(name).ok_or_else(|| {
        let all: Vec<&str> = HookEvent::ALL.iter().map(|e| e.as_str()).collect();
        format!("Unknown hook event {name:?}. Events: {}.", all.join(", "))
    })
}

/// Apply the change and reload the session so the hooks take effect.
async fn saved(d: &mut Driver, note: String) -> Exec {
    if d.can_switch() {
        if let Err(e) = d.switch(Switch::Reload).await {
            return err(format!("{note} Could not reload the session: {e}"));
        }
        return ok(format!("{note} It applies now."));
    }
    ok(format!("{note} It applies to new sessions."))
}

/// The array at `hooks.<Event>` in the settings layer of `scope`.
fn layer_list(d: &Driver, scope: Scope, event: HookEvent) -> Vec<Value> {
    d.info
        .settings
        .layers
        .iter()
        .find(|l| l.source == scope.source())
        .and_then(|l| l.value.pointer(&format!("/hooks/{}", event.as_str())))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

async fn add(d: &mut Driver, words: &[String]) -> Exec {
    let mut scope = Scope::Local;
    let mut timeout: Option<f64> = None;
    let mut positional = vec![];
    let mut it = words.iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "--scope" => match it.next().and_then(|s| Scope::parse(s)) {
                Some(s) => scope = s,
                None => return err("--scope takes local, project or user."),
            },
            "--timeout" => match it.next().and_then(|t| t.parse::<f64>().ok()).filter(|t| *t > 0.0) {
                Some(t) => timeout = Some(t),
                None => return err("--timeout takes a number of seconds."),
            },
            _ => positional.push(w.as_str()),
        }
    }
    let [event, matcher, command @ ..] = positional.as_slice() else { return err(USAGE) };
    let command = command.join(" ");
    if command.trim().is_empty() {
        return err(USAGE);
    }
    let event = match event_of(event) {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    let mut hook = json!({"type": "command", "command": command});
    if let Some(t) = timeout {
        hook["timeout"] = json!(t);
    }
    let mut entry = json!({"hooks": [hook]});
    let matcher = matcher.trim();
    if !matcher.is_empty() && matcher != "*" {
        entry["matcher"] = json!(matcher);
    }
    let mut list = layer_list(d, scope, event);
    list.push(entry);
    let note = match save(d, scope, &["hooks", event.as_str()], Some(Value::Array(list))) {
        Ok(n) => n,
        Err(e) => return err(format!("Could not add the hook: {e}")),
    };
    let what = if matcher.is_empty() || matcher == "*" { String::new() } else { format!(" for {matcher}") };
    saved(d, format!("Added a {} hook{what}: {command}. {note}", event.as_str())).await
}

async fn remove(d: &mut Driver, words: &[String]) -> Exec {
    let [event, n] = words else { return err(USAGE) };
    let event = match event_of(event) {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    let list = entries(d, event);
    let Some(e) = n.parse::<usize>().ok().filter(|n| *n >= 1).and_then(|n| list.get(n - 1)) else {
        return err(format!("No {} hook {n}. /hooks lists them.", event.as_str()));
    };
    let Some((source, mi, hi)) = e.at else {
        return err("That hook comes from a plugin; remove it from the plugin, or turn the plugin off.");
    };
    let scope = match source {
        SettingSource::User => Scope::User,
        SettingSource::Project => Scope::Project,
        SettingSource::Local => Scope::Local,
        SettingSource::Flag | SettingSource::Managed => {
            return err(format!("That hook comes from {} settings, which /hooks can't change.", e.source));
        }
    };
    let mut list = layer_list(d, scope, event);
    let Some(m) = list.get_mut(mi) else { return err("The settings changed; run /hooks again.") };
    if let Some(hooks) = m.get_mut("hooks").and_then(Value::as_array_mut) {
        if hi < hooks.len() {
            hooks.remove(hi);
        }
        if hooks.is_empty() {
            list.remove(mi);
        }
    }
    let value = if list.is_empty() { None } else { Some(Value::Array(list)) };
    let command = e.command.clone();
    let note = match save(d, scope, &["hooks", event.as_str()], value) {
        Ok(n) => n,
        Err(e) => return err(format!("Could not remove the hook: {e}")),
    };
    saved(d, format!("Removed the {} hook {command}. {note}", event.as_str())).await
}

pub(super) async fn run(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return ok(listing(d));
    }
    let Some(words) = shlex::split(args) else { return err(USAGE) };
    match words.first().map(String::as_str) {
        Some("add") => add(d, &words[1..]).await,
        Some("remove") => remove(d, &words[1..]).await,
        _ => err(USAGE),
    }
}

/// The form that adds a hook.
fn add_form() -> Form {
    let events: Vec<&str> = HookEvent::ALL.iter().map(|e| e.as_str()).collect();
    Form {
        title: "Add a hook".into(),
        fields: vec![
            Field::choice("Event", &events),
            Field::text("Matcher (tool name, empty for all)", ""),
            Field::text("Command", ""),
            Field::choice("Save in", &["local", "project", "user"]),
        ],
        template: "/hooks add {0} {1} {2} --scope {3}".into(),
    }
}

/// The hooks editor: each event's hooks with their source; Enter on one removes it, and
/// "Add hook…" opens a form.
pub(super) fn screen(d: &Driver) -> Screen {
    let mut rows = vec![Row {
        spans: vec![Span { text: "+ Add hook…".into(), tone: Tone::Accent }],
        action: Some(RowAction::Form(add_form())),
    }];
    if disabled(d) {
        rows.push(Row::text("Hooks are disabled (--bare, --safe-mode or disableAllHooks).", Tone::Dim));
    }
    for event in HookEvent::ALL {
        let list = entries(d, event);
        rows.push(Row::blank());
        rows.push(Row {
            spans: vec![
                Span { text: event.as_str().into(), tone: Tone::Bold },
                Span {
                    text: format!("  {}", if list.is_empty() { "none".into() } else { list.len().to_string() }),
                    tone: Tone::Dim,
                },
            ],
            action: None,
        });
        for (i, e) in list.iter().enumerate() {
            let removable =
                matches!(e.at, Some((SettingSource::User | SettingSource::Project | SettingSource::Local, _, _)));
            rows.push(Row {
                spans: vec![
                    Span { text: format!("  {}. [{}] ", i + 1, matcher_label(&e.matcher)), tone: Tone::Plain },
                    Span { text: e.command.clone(), tone: Tone::Plain },
                    Span { text: format!("  {} · {}s", e.source, e.timeout), tone: Tone::Dim },
                ],
                action: removable.then(|| RowAction::Confirm {
                    question: format!("Remove the {} hook {}?", event.as_str(), e.command),
                    command: format!("/hooks remove {} {}", event.as_str(), i + 1),
                }),
            });
        }
    }
    Screen { title: "Hooks · Enter on a hook removes it".into(), rows }
}
