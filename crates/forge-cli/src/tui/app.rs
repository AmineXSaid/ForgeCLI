//! The UI's state: what is being typed, what streams in, the dialog that is
//! open, and the lines waiting to go into the terminal's scrollback. Keys and
//! session events change it; the screen is drawn from it.

use forge_core::glyphs;
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use forge_core::commands::picker::{Choice, Pick, Picker};
use forge_core::commands::screens::{FieldKind, Form, RowAction, Screen};
use forge_engine::{EngineEvent, NoticeLevel, PermissionAnswer, PermissionPrompt};
use forge_permissions::{PermissionMode, Suggestion};
use forge_types::{ContentBlock, Delta, StreamEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::editor::Editor;
use super::text::{Markdown, Theme};

/// Under a tool call or a command: its result.
const RESULT_MARK: &str = "  ↳  ";

/// A slash command for the `/` menu.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub args: String,
    pub description: String,
}

/// What the status line shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusView {
    pub model: String,
    pub mode: String,
    pub cwd: String,
    pub cost: f64,
    /// Context used, in percent of the model's window.
    pub context_pct: Option<u8>,
}

/// From the session task to the UI.
pub enum UiEvent {
    Engine(EngineEvent),
    /// A question for the person (permission, AskUserQuestion, plan approval).
    Ask {
        prompt: PermissionPrompt,
        reply: oneshot::Sender<PermissionAnswer>,
    },
    /// A local answer (a slash command) or an error to show.
    Reply {
        text: String,
        is_error: bool,
    },
    Status(StatusView),
    Commands(Vec<CommandInfo>),
    /// The project's files, for `@` completion.
    Files(Vec<String>),
    /// The `theme` setting changed (`/theme`).
    Theme(String),
    /// Put this text on the clipboard (`/copy`).
    Copy(String),
    /// The `statusLine` command's output, or `None` when there is none.
    StatusLine(Option<String>),
    /// Choices for a command typed without its argument (`/model`, `/resume`, ...).
    Picker(Picker),
    /// A screen for a command typed without arguments (`/diff`, `/context`, `/hooks`, `/agents`).
    Screen(Screen),
    /// The session started work by itself (a scheduled task).
    Busy,
    /// The session is ready for the next input.
    Idle,
    /// The session ended (`/exit`).
    Exit,
}

/// What the UI loop does for the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Send(String),
    /// Ask the session for the picker of this command text (a second step).
    Picker(String),
    Interrupt,
    SetMode(PermissionMode),
    Exit,
}

/// One AskUserQuestion question.
#[derive(Debug, Clone)]
struct Question {
    question: String,
    options: Vec<(String, String)>,
    multi: bool,
}

#[derive(Debug)]
enum DialogKind {
    Permission {
        always: Option<String>,
    },
    Plan,
    Questions {
        questions: Vec<Question>,
        at: usize,
        answers: serde_json::Map<String, Value>,
        picked: Vec<bool>,
    },
    /// Typing filters the rows.
    Picker {
        picker: Picker,
        filter: String,
    },
    /// A screen: scrolling rows, some with an action (docs/TUI.md, "Screens").
    Viewer {
        screen: Screen,
        /// The highlighted row.
        cursor: usize,
        /// The first row shown.
        top: usize,
        /// Rows jumped from, for Esc.
        back: Vec<usize>,
    },
    /// Inputs that fill in a command.
    Form {
        form: Form,
        /// The field being edited.
        at: usize,
    },
}

/// An open question for the person.
pub struct Dialog {
    /// The question being answered (none for a picker).
    prompt: Option<PermissionPrompt>,
    reply: Option<oneshot::Sender<PermissionAnswer>>,
    kind: DialogKind,
    selected: usize,
    /// "Type an answer" in a question.
    typing: Option<Editor>,
}

/// Ctrl+R: a search back through prompt history.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub query: String,
    /// The history entry shown.
    pub hit: Option<usize>,
    /// What was typed before the search, for Esc.
    draft: String,
}

/// The keyboard shortcuts (`?` on an empty prompt, `/keybindings`).
pub const KEYS: &[(&str, &str, Option<&str>)] = &[
    ("Enter", "Send (queued while a turn runs)", Some("submit")),
    ("Shift+Enter, Alt+Enter, Ctrl+J, \\ Enter", "New line", Some("newline")),
    ("Esc", "Interrupt the turn; close a menu; cancel a dialog", Some("cancel")),
    ("Esc Esc", "On an empty prompt: rewind (/rewind)", None),
    ("Ctrl+C", "Clear the input; interrupt; twice on an empty prompt: exit", Some("interrupt")),
    ("Ctrl+D", "Exit (empty prompt)", Some("exit")),
    ("Shift+Tab", "Cycle the permission mode: default, accept edits, plan", Some("cycleMode")),
    ("Up / Down", "Move between lines; earlier prompts", None),
    ("Ctrl+R", "Search earlier prompts", Some("historySearch")),
    ("Tab", "Complete a / command or an @ path", Some("complete")),
    ("@", "Mention a file (a menu of project paths)", None),
    ("!", "At the start: run a shell command", None),
    ("/", "At the start: a command (the menu lists them)", None),
    ("Ctrl+A / Ctrl+E, Home / End", "Start / end of line", None),
    ("Ctrl+W, Alt+Backspace", "Delete the word before the cursor", Some("deleteWord")),
    ("Ctrl+U / Ctrl+K", "Delete to the start / end of the line", None),
    ("Alt+B / Alt+F, Ctrl+Left / Ctrl+Right", "Word left / right", None),
    ("Ctrl+L", "Redraw the screen", Some("redraw")),
];

/// The shortcuts in effect, one per line: the defaults, or with `keybindings.json`
/// the keys each action has now, and the user's own bindings.
pub fn keys_text(keymap: &super::keys::Keymap) -> String {
    let rows: Vec<(String, String)> = KEYS
        .iter()
        .map(|(k, what, action)| {
            let keys = match action {
                Some(a) if !keymap.is_default() => keymap.keys_for(a).join(", "),
                _ => k.to_string(),
            };
            (keys, what.to_string())
        })
        .collect();
    let w = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let mut lines: Vec<String> = rows.iter().map(|(k, what)| format!("{k:<w$}  {what}")).collect();
    if !keymap.is_default() {
        lines.push(String::new());
        lines.push(format!("Custom bindings from {}.", forge_config::config_dir().join("keybindings.json").display()));
        let unbound = keymap.unbound();
        if !unbound.is_empty() {
            lines.push(format!("Unbound: {}.", unbound.join(", ")));
        }
    }
    lines.join("\n")
}

const TERMINAL_SETUP: &str = "Shift+Enter starts a new line when the terminal reports it as its own key. Forge asks \
for this through the keyboard protocol that kitty, WezTerm, foot, Ghostty, Alacritty and iTerm2 (with \
\"Report keys using CSI u\" on) support.\n\nWhere it isn't available, these always start a new line: Alt+Enter \
(Option+Enter on macOS, with \"Use Option as Meta key\" on), Ctrl+J, or \\ then Enter.\n\nInside tmux, add \
`set -s extended-keys on` and `set -as terminal-features 'xterm*:extkeys'` to ~/.tmux.conf.";

/// `/terminal-setup`: whether Shift+Enter works here, and the ways around it.
pub fn terminal_setup_text(keyboard_protocol: bool) -> String {
    let now = if keyboard_protocol {
        "This terminal has the keyboard protocol on: Shift+Enter works."
    } else {
        "This terminal didn't turn the keyboard protocol on, so Shift+Enter may arrive as Enter."
    };
    format!("{now}\n\n{TERMINAL_SETUP}")
}

/// Rows the `@` menu offers at most.
const FILE_MATCHES: usize = 50;

/// Two presses of Esc or Ctrl-C within this make the second one count.
const DOUBLE_PRESS: Duration = Duration::from_millis(800);

pub struct App {
    pub theme: Theme,
    pub editor: Editor,
    pub status: StatusView,
    pub busy: bool,
    pub busy_since: Option<Instant>,
    /// What the session is doing, for the spinner: "Thinking", "Running Bash", ...
    pub activity: String,
    pub queued: VecDeque<String>,
    /// Lines waiting to be written into scrollback.
    pub pending: Vec<Line<'static>>,
    /// The current answer's unfinished last line.
    pub live: String,
    md: Markdown,
    /// The next answer line is the first of its block (it gets the marker).
    first_line: bool,
    /// Text streamed for the current message (its complete copy isn't written again).
    streamed: bool,
    pub dialog: Option<Dialog>,
    commands: Vec<CommandInfo>,
    files: Vec<String>,
    pub search: Option<Search>,
    pub menu_selected: usize,
    menu_dismissed: bool,
    last_esc: Option<Instant>,
    last_ctrl_c: Option<Instant>,
    /// A short hint in the status line ("Press Ctrl-C again to exit").
    pub hint: Option<(String, Instant)>,
    pub exit: bool,
    pub dirty: bool,
    /// Colour is allowed at all (not `NO_COLOR` or `--color never`).
    color_ok: bool,
    /// Text for the loop to put on the clipboard.
    pub clipboard: Option<String>,
    /// The `statusLine` command's output.
    pub status_text: Option<String>,
    /// Rows a screen showed last time it was drawn (PageUp/PageDown move by this).
    pub viewer_page: std::cell::Cell<usize>,
    /// Key bindings in effect (`keybindings.json`).
    pub keymap: super::keys::Keymap,
    /// `/focus`: tool calls and their results stay out of the scrollback.
    pub focus: bool,
    /// A screen or picker waiting for an open question to be answered.
    held: Option<Dialog>,
}

/// The main argument of a tool call, for one line.
pub fn summarize(tool: &str, input: &Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "LS" => "path",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Task" => "description",
        "Skill" => "skill",
        _ => "",
    };
    let v = input.get(key).and_then(Value::as_str).map(str::to_string).or_else(|| match tool {
        "TodoWrite" => input["todos"].as_array().map(|a| format!("{} todos", a.len())),
        _ => input.as_object().and_then(|o| o.values().find_map(Value::as_str).map(str::to_string)),
    });
    let v = v.unwrap_or_default();
    let one = v.lines().next().unwrap_or("");
    if one.chars().count() > 100 {
        format!("{}…", one.chars().take(100).collect::<String>())
    } else {
        one.to_string()
    }
}

fn rule_text(s: &Suggestion) -> Option<String> {
    match s {
        Suggestion::AddRules { rules, .. } => {
            Some(rules.iter().map(|r| r.to_rule_string()).collect::<Vec<_>>().join(", "))
        }
        Suggestion::AddDirectories { directories, .. } => Some(directories.join(", ")),
        Suggestion::SetMode { mode, .. } => Some(format!("{mode} mode")),
    }
}

const MODES: [PermissionMode; 3] = [PermissionMode::Default, PermissionMode::AcceptEdits, PermissionMode::Plan];

impl App {
    pub fn new(theme: Theme, history: Vec<String>) -> Self {
        App {
            theme,
            editor: Editor::with_history(history),
            status: StatusView::default(),
            busy: false,
            busy_since: None,
            activity: String::new(),
            queued: VecDeque::new(),
            pending: vec![],
            live: String::new(),
            md: Markdown::default(),
            first_line: true,
            streamed: false,
            dialog: None,
            commands: vec![],
            files: vec![],
            search: None,
            menu_selected: 0,
            menu_dismissed: false,
            last_esc: None,
            last_ctrl_c: None,
            hint: None,
            exit: false,
            dirty: true,
            color_ok: theme.color,
            clipboard: None,
            status_text: None,
            viewer_page: std::cell::Cell::new(10),
            keymap: Default::default(),
            focus: false,
            held: None,
        }
    }

    // ---- scrollback ----

    /// A blank line between items, unless one is already there.
    fn gap(&mut self) {
        let blank = |l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty());
        if self.pending.last().is_none_or(|l| !blank(l)) {
            self.pending.push(Line::default());
        }
    }

    pub fn take_pending(&mut self) -> Vec<Line<'static>> {
        std::mem::take(&mut self.pending)
    }

    fn commit_text_line(&mut self, raw: &str) {
        let t = self.theme;
        let mut line = self.md.line(raw, &t);
        let marker =
            if self.first_line { Span::styled(format!("{} ", glyphs::ANSWER), t.accent()) } else { Span::raw("  ") };
        if self.first_line {
            self.gap();
        }
        self.first_line = false;
        line.spans.insert(0, marker);
        self.pending.push(line);
    }

    /// The unfinished answer line as it will look in scrollback.
    pub fn live_line(&self) -> Option<Line<'static>> {
        if self.live.is_empty() {
            return None;
        }
        let t = self.theme;
        let mut line = self.md.clone().line(&self.live, &t);
        let marker =
            if self.first_line { Span::styled(format!("{} ", glyphs::ANSWER), t.accent()) } else { Span::raw("  ") };
        line.spans.insert(0, marker);
        Some(line)
    }

    fn flush_live(&mut self) {
        if !self.live.is_empty() {
            let rest = std::mem::take(&mut self.live);
            self.commit_text_line(&rest);
        }
    }

    /// Write the person's prompt into scrollback.
    pub fn echo_prompt(&mut self, text: &str) {
        self.gap();
        let st = self.theme.user();
        for (i, l) in text.lines().enumerate() {
            let marker = if i == 0 { "> " } else { "  " };
            self.pending.push(Line::from(Span::styled(format!("{marker}{l} "), st)));
        }
    }

    fn reply_lines(&mut self, text: &str, is_error: bool) {
        let st = if is_error { self.theme.error() } else { Style::default() };
        self.gap();
        for (i, l) in text.lines().enumerate() {
            let marker = if i == 0 { RESULT_MARK } else { "     " };
            self.pending
                .push(Line::from(vec![Span::styled(marker, self.theme.dim()), Span::styled(l.to_string(), st)]));
        }
    }

    fn notice(&mut self, level: NoticeLevel, text: &str) {
        let st = match level {
            NoticeLevel::Info => self.theme.dim(),
            NoticeLevel::Warning => self.theme.warning(),
            NoticeLevel::Error => self.theme.error(),
        };
        self.gap();
        for l in text.lines() {
            self.pending.push(Line::from(Span::styled(format!("  {l}"), st)));
        }
    }

    // ---- session events ----

    pub fn on_event(&mut self, ev: UiEvent) {
        self.dirty = true;
        match ev {
            UiEvent::Engine(e) => self.on_engine(e),
            UiEvent::Ask { prompt, reply } => self.open_dialog(prompt, reply),
            UiEvent::Reply { text, is_error } => {
                if !text.trim().is_empty() {
                    self.reply_lines(&text, is_error);
                }
            }
            UiEvent::Status(s) => self.status = s,
            UiEvent::Commands(c) => self.commands = c,
            UiEvent::Files(f) => self.files = f,
            UiEvent::Theme(name) => {
                // The session's /color stays.
                let accent = self.theme.accent;
                self.theme = Theme::named(&name, self.color_ok);
                self.theme.accent = accent;
            }
            UiEvent::Copy(text) => self.clipboard = Some(text),
            UiEvent::StatusLine(text) => self.status_text = text,
            UiEvent::Picker(picker) => self.open_picker(picker),
            UiEvent::Screen(screen) => self.open_screen(screen),
            UiEvent::Busy => {
                if !self.busy {
                    self.busy = true;
                    self.busy_since = Some(Instant::now());
                    self.activity = "Running a scheduled task".into();
                    self.first_line = true;
                }
            }
            UiEvent::Idle => {
                self.flush_live();
                self.busy = false;
                self.busy_since = None;
                self.activity.clear();
            }
            UiEvent::Exit => self.exit = true,
        }
    }

    /// The next queued message, once the session is idle.
    pub fn next_queued(&mut self) -> Option<String> {
        if self.busy {
            return None;
        }
        let text = self.queued.pop_front()?;
        Some(self.start(text))
    }

    /// A message goes to the session: echo it and mark the session busy.
    fn start(&mut self, text: String) -> String {
        self.echo_prompt(&text);
        self.busy = true;
        self.busy_since = Some(Instant::now());
        self.activity = "Working".into();
        self.first_line = true;
        text
    }

    fn on_engine(&mut self, e: EngineEvent) {
        match e {
            // Sub-agents' own messages stay in their transcripts.
            EngineEvent::Stream { parent_tool_use_id: Some(_), .. }
            | EngineEvent::Assistant { parent_tool_use_id: Some(_), .. }
            | EngineEvent::User { parent_tool_use_id: Some(_), .. } => {}
            EngineEvent::Stream { event, .. } => match event {
                StreamEvent::MessageStart { .. } => self.streamed = false,
                StreamEvent::ContentBlockStart { content_block: ContentBlock::Text { .. }, .. } => {
                    self.flush_live();
                    self.first_line = true;
                    self.md.reset();
                    self.activity = "Writing".into();
                }
                StreamEvent::ContentBlockDelta { delta: Delta::TextDelta { text }, .. } => {
                    self.streamed = true;
                    self.live.push_str(&text);
                    while let Some(i) = self.live.find('\n') {
                        let line: String = self.live[..i].to_string();
                        self.live.drain(..=i);
                        self.commit_text_line(&line);
                    }
                }
                StreamEvent::ContentBlockDelta { delta: Delta::ThinkingDelta { .. }, .. } => {
                    self.activity = "Thinking".into();
                }
                StreamEvent::ContentBlockStop { .. } | StreamEvent::MessageStop => self.flush_live(),
                _ => {}
            },
            EngineEvent::Assistant { message, .. } => {
                self.flush_live();
                for b in &message.content {
                    match b {
                        ContentBlock::Text { text, .. } if !self.streamed && !text.trim().is_empty() => {
                            self.first_line = true;
                            self.md.reset();
                            for l in text.lines() {
                                self.commit_text_line(l);
                            }
                        }
                        ContentBlock::ToolUse { name, .. } if self.focus => self.activity = format!("Running {name}"),
                        ContentBlock::ToolUse { name, input, .. } => {
                            self.gap();
                            let t = self.theme;
                            self.pending.push(Line::from(vec![
                                Span::styled(format!("{} ", glyphs::TOOL), t.success()),
                                Span::styled(name.clone(), t.bold()),
                                Span::raw(format!("({})", summarize(name, input))),
                            ]));
                            self.activity = format!("Running {name}");
                        }
                        _ => {}
                    }
                }
                self.streamed = false;
            }
            EngineEvent::User { message, is_meta: false, .. } => {
                if self.focus {
                    self.activity = "Working".into();
                    return;
                }
                for b in &message.content {
                    if let ContentBlock::ToolResult { content, is_error, .. } = b {
                        let text = content.to_text();
                        let first: String = text.lines().next().unwrap_or("").chars().take(160).collect();
                        let more = text.lines().count().saturating_sub(1);
                        let more = if more > 0 { format!(" (+{more} lines)") } else { String::new() };
                        let st = if *is_error == Some(true) { self.theme.error() } else { self.theme.dim() };
                        self.pending.push(Line::from(vec![
                            Span::styled(RESULT_MARK, self.theme.dim()),
                            Span::styled(format!("{first}{more}"), st),
                        ]));
                        self.activity = "Working".into();
                    }
                }
            }
            EngineEvent::PromptAccepted { message, .. } => {
                let names: Vec<String> = message
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .flat_map(forge_agents::attach::attached_paths)
                    .map(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(p))
                    .collect();
                if !names.is_empty() {
                    let line = format!("  (attached: {})", names.join(", "));
                    self.pending.push(Line::from(Span::styled(line, self.theme.dim())));
                }
            }
            EngineEvent::Notice { level, text } => self.notice(level, &text),
            EngineEvent::System { subtype, data } => match subtype.as_str() {
                "compact_boundary" => self.notice(NoticeLevel::Info, "Conversation compacted."),
                "model_fallback" => self.notice(
                    NoticeLevel::Warning,
                    &format!(
                        "Switched to {} for this turn.",
                        data.get("to").and_then(Value::as_str).unwrap_or("another model")
                    ),
                ),
                _ => {}
            },
            EngineEvent::ToolProgress { tool_name, elapsed_secs, .. } => {
                self.activity = format!("Running {tool_name} ({}s)", elapsed_secs as u64);
            }
            _ => {}
        }
    }

    // ---- dialogs ----

    fn open_dialog(&mut self, prompt: PermissionPrompt, reply: oneshot::Sender<PermissionAnswer>) {
        self.flush_live();
        let kind = match prompt.tool_name.as_str() {
            "AskUserQuestion" => {
                let questions: Vec<Question> = prompt.input["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|q| Question {
                        question: q["question"].as_str().unwrap_or("").to_string(),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|o| {
                                (
                                    o["label"].as_str().unwrap_or("").to_string(),
                                    o["description"].as_str().unwrap_or("").to_string(),
                                )
                            })
                            .collect(),
                        multi: q["multiSelect"].as_bool().unwrap_or(false),
                    })
                    .collect();
                let picked = vec![false; questions.first().map(|q| q.options.len()).unwrap_or(0)];
                DialogKind::Questions { questions, at: 0, answers: serde_json::Map::new(), picked }
            }
            "ExitPlanMode" => {
                // The plan goes into scrollback, so the dialog stays small.
                self.gap();
                self.pending.push(Line::from(Span::styled("Here is the plan:", self.theme.bold())));
                let plan = prompt.input["plan"].as_str().unwrap_or("").to_string();
                let mut md = Markdown::default();
                for l in plan.lines() {
                    let mut line = md.line(l, &self.theme);
                    line.spans.insert(0, Span::raw("  "));
                    self.pending.push(line);
                }
                DialogKind::Plan
            }
            _ => DialogKind::Permission { always: prompt.suggestions.first().and_then(rule_text) },
        };
        self.dialog = Some(Dialog { prompt: Some(prompt), reply: Some(reply), kind, selected: 0, typing: None });
    }

    fn answer(&mut self, a: PermissionAnswer) {
        if let Some(mut d) = self.dialog.take() {
            if let Some(tx) = d.reply.take() {
                let _ = tx.send(a);
            }
        }
        // A screen or picker that arrived while the question was open shows now.
        self.dialog = self.held.take();
    }

    /// Show a screen or picker, unless a question is waiting for its answer: replacing that
    /// dialog would drop its reply, which the engine reads as "the UI closed" (deny and
    /// interrupt). It is held until the question is answered.
    fn show(&mut self, d: Dialog) {
        if self.dialog.as_ref().is_some_and(|open| open.reply.is_some()) {
            self.held = Some(d);
        } else {
            self.dialog = Some(d);
        }
    }

    /// The rows of the open dialog: (label, description).
    pub fn dialog_options(&self) -> Vec<(String, String)> {
        let Some(d) = &self.dialog else { return vec![] };
        match &d.kind {
            DialogKind::Permission { always } => {
                let mut v = vec![("Yes".to_string(), String::new())];
                if let Some(r) = always {
                    v.push((format!("Yes, and don't ask again for {r}"), String::new()));
                }
                v.push(("No, and tell Forge what to do instead".to_string(), "esc".to_string()));
                v
            }
            DialogKind::Plan => vec![
                ("Yes, and accept edits without asking".into(), String::new()),
                ("Yes, and ask before each edit".into(), String::new()),
                ("No, keep planning".into(), "esc".into()),
            ],
            DialogKind::Questions { questions, at, picked, .. } => {
                let q = &questions[*at];
                let mut v: Vec<(String, String)> = q
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, (l, desc))| {
                        let label =
                            if q.multi { format!("[{}] {l}", if picked[i] { "x" } else { " " }) } else { l.clone() };
                        (label, desc.clone())
                    })
                    .collect();
                v.push(("Type an answer".into(), String::new()));
                v
            }
            DialogKind::Viewer { .. } | DialogKind::Form { .. } => vec![],
            DialogKind::Picker { .. } => self
                .picker_rows()
                .into_iter()
                .map(|c| (if c.current { format!("{} ✔", c.label) } else { c.label.clone() }, c.detail.clone()))
                .collect(),
        }
    }

    /// The open picker's rows that match its filter.
    fn picker_rows(&self) -> Vec<&forge_core::commands::picker::Choice> {
        let Some(Dialog { kind: DialogKind::Picker { picker, filter }, .. }) = &self.dialog else { return vec![] };
        let f = filter.to_lowercase();
        picker
            .choices
            .iter()
            .filter(|c| f.is_empty() || c.label.to_lowercase().contains(&f) || c.detail.to_lowercase().contains(&f))
            .collect()
    }

    /// The dialog's title and body lines.
    pub fn dialog_text(&self) -> (String, Vec<String>) {
        let Some(d) = &self.dialog else { return (String::new(), vec![]) };
        match &d.kind {
            DialogKind::Picker { picker, filter } => {
                let body = if filter.is_empty() {
                    vec!["Type to filter".to_string()]
                } else {
                    vec![format!("Filter: {filter}")]
                };
                (picker.title.clone(), body)
            }
            DialogKind::Permission { .. } => {
                let Some(p) = &d.prompt else { return (String::new(), vec![]) };
                let arg = summarize(&p.tool_name, &p.input);
                let mut body = vec![];
                if p.tool_name == "Bash" {
                    body.extend(p.input["command"].as_str().unwrap_or("").lines().take(6).map(|l| format!("  {l}")));
                } else if !arg.is_empty() {
                    body.push(format!("  {arg}"));
                }
                if !p.reason.is_empty() {
                    body.push(p.reason.clone());
                }
                (format!("Allow {}?", p.tool_name), body)
            }
            DialogKind::Plan => ("Go ahead with this plan?".into(), vec![]),
            DialogKind::Viewer { screen, .. } => (screen.title.clone(), vec![]),
            DialogKind::Form { form, .. } => (form.title.clone(), vec![]),
            DialogKind::Questions { questions, at, .. } => {
                let q = &questions[*at];
                let n = questions.len();
                let title = if n > 1 { format!("{} ({}/{n})", q.question, at + 1) } else { q.question.clone() };
                let mut body = vec![];
                if let Some(e) = &d.typing {
                    body.push(format!("> {}", e.text()));
                }
                (title, body)
            }
        }
    }

    pub fn dialog_selected(&self) -> usize {
        self.dialog.as_ref().map(|d| d.selected).unwrap_or(0)
    }

    fn dialog_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if matches!(self.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::Viewer { .. })) {
            return self.viewer_key(key);
        }
        if matches!(self.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::Form { .. })) {
            return self.form_key(key);
        }
        let n = self.dialog_options().len();
        let Some(d) = self.dialog.as_mut() else { return vec![] };
        if let DialogKind::Picker { filter, .. } = &mut d.kind {
            match key.code {
                KeyCode::Up => d.selected = d.selected.checked_sub(1).unwrap_or(n.saturating_sub(1)),
                KeyCode::Down | KeyCode::Tab => d.selected = (d.selected + 1) % n.max(1),
                KeyCode::Enter if n > 0 => {
                    let i = d.selected.min(n - 1);
                    return self.choose(i);
                }
                KeyCode::Esc => self.dialog = None,
                KeyCode::Backspace => {
                    filter.pop();
                    d.selected = 0;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    filter.push(c);
                    d.selected = 0;
                }
                _ => {}
            }
            return vec![];
        }
        // Typing a free-form answer.
        if let Some(e) = d.typing.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let text = e.text();
                    d.typing = None;
                    if !text.trim().is_empty() {
                        self.answer_question(text);
                    }
                }
                KeyCode::Esc => d.typing = None,
                KeyCode::Backspace => e.backspace(),
                KeyCode::Left => e.left(),
                KeyCode::Right => e.right(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => e.insert(&c.to_string()),
                _ => {}
            }
            return vec![];
        }
        match key.code {
            KeyCode::Up => d.selected = d.selected.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            KeyCode::Down | KeyCode::Tab => d.selected = (d.selected + 1) % n.max(1),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let i = c as usize - '1' as usize;
                if i < n {
                    d.selected = i;
                    return self.choose(i);
                }
            }
            KeyCode::Char(' ') => {
                if let DialogKind::Questions { questions, at, picked, .. } = &mut d.kind {
                    if questions[*at].multi && d.selected < picked.len() {
                        picked[d.selected] = !picked[d.selected];
                    }
                }
            }
            KeyCode::Enter => {
                let i = d.selected;
                return self.choose(i);
            }
            KeyCode::Esc => return self.choose(n.saturating_sub(1)),
            _ => {}
        }
        vec![]
    }

    fn choose(&mut self, i: usize) -> Vec<Action> {
        let n = self.dialog_options().len();
        let Some(d) = self.dialog.as_mut() else { return vec![] };
        let allow = |updated_permissions| PermissionAnswer::Allow { updated_input: None, updated_permissions };
        let stop = |message: &str| PermissionAnswer::Deny { message: message.to_string(), interrupt: true };
        match &mut d.kind {
            DialogKind::Picker { .. } => {
                let Some(pick) = self.picker_rows().get(i).map(|c| c.pick.clone()) else { return vec![] };
                self.dialog = None;
                return match pick {
                    Pick::Run(text) => self.submit(text),
                    Pick::RunThenEdit { command, edit } => {
                        let a = self.submit(command);
                        self.editor.set(&edit);
                        a
                    }
                    Pick::Step(text) => vec![Action::Picker(text)],
                    Pick::Edit(text) => {
                        self.editor.set(&text);
                        vec![]
                    }
                };
            }
            DialogKind::Viewer { .. } | DialogKind::Form { .. } => {}
            DialogKind::Permission { always } => {
                let always_row = always.is_some();
                let suggestions = d.prompt.as_ref().map(|p| p.suggestions.clone()).unwrap_or_default();
                let a = match (i, always_row) {
                    (0, _) => allow(vec![]),
                    (1, true) => allow(suggestions.iter().filter_map(|s| serde_json::to_value(s).ok()).collect()),
                    _ => stop("The user said no. Wait for them to say what to do instead."),
                };
                self.answer(a);
            }
            DialogKind::Plan => {
                let a = match i {
                    0 => allow(vec![json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})]),
                    1 => allow(vec![]),
                    _ => stop("The user wants to keep planning. Wait for their feedback on the plan."),
                };
                self.answer(a);
            }
            DialogKind::Questions { questions, at, picked, .. } => {
                if i + 1 == n {
                    // "Type an answer"; Esc on it declines.
                    if d.selected + 1 == n && d.typing.is_none() {
                        d.typing = Some(Editor::default());
                    }
                    return vec![];
                }
                let q = &questions[*at];
                let answer = if q.multi {
                    let mut chosen: Vec<String> =
                        q.options.iter().zip(picked.iter()).filter(|(_, p)| **p).map(|(o, _)| o.0.clone()).collect();
                    if chosen.is_empty() {
                        chosen.push(q.options[i].0.clone());
                    }
                    chosen.join(", ")
                } else {
                    q.options[i].0.clone()
                };
                self.answer_question(answer);
            }
        }
        vec![]
    }

    fn answer_question(&mut self, answer: String) {
        let Some(d) = self.dialog.as_mut() else { return };
        let DialogKind::Questions { questions, at, answers, picked } = &mut d.kind else { return };
        answers.insert(questions[*at].question.clone(), json!(answer));
        if *at + 1 < questions.len() {
            *at += 1;
            *picked = vec![false; questions[*at].options.len()];
            d.selected = 0;
            return;
        }
        let mut input = d.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_default();
        input["answers"] = Value::Object(std::mem::take(answers));
        self.answer(PermissionAnswer::Allow { updated_input: Some(input), updated_permissions: vec![] });
    }

    fn open_picker(&mut self, picker: Picker) {
        self.flush_live();
        let selected = picker.choices.iter().position(|c| c.current).unwrap_or(0);
        self.show(Dialog {
            prompt: None,
            reply: None,
            kind: DialogKind::Picker { picker, filter: String::new() },
            selected,
            typing: None,
        });
    }

    // ---- screens ----

    /// Show a screen (`/diff`, `/context`, `/hooks`, `/agents`).
    pub fn open_screen(&mut self, screen: Screen) {
        self.flush_live();
        let kind = DialogKind::Viewer { screen, cursor: 0, top: 0, back: vec![] };
        self.show(Dialog { prompt: None, reply: None, kind, selected: 0, typing: None });
    }

    /// The open screen: its rows, the highlighted row and the first row shown.
    pub fn viewer(&self) -> Option<(&Screen, usize, usize)> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Viewer { screen, cursor, top, .. } => Some((screen, *cursor, *top)),
            _ => None,
        }
    }

    /// The open form and the field being edited.
    pub fn form(&self) -> Option<(&Form, usize)> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Form { form, at } => Some((form, *at)),
            _ => None,
        }
    }

    fn viewer_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let page = self.viewer_page.get().max(1);
        let Some(Dialog { kind: DialogKind::Viewer { screen, cursor, top, back }, .. }) = self.dialog.as_mut() else {
            return vec![];
        };
        let last = screen.rows.len().saturating_sub(1);
        match key.code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(last),
            KeyCode::PageUp => *cursor = cursor.saturating_sub(page),
            KeyCode::PageDown | KeyCode::Char(' ') => *cursor = (*cursor + page).min(last),
            KeyCode::Home => *cursor = 0,
            KeyCode::End => *cursor = last,
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => match back.pop() {
                Some(from) if key.code != KeyCode::Char('q') => *cursor = from,
                _ => {
                    self.dialog = None;
                    return vec![];
                }
            },
            KeyCode::Enter | KeyCode::Right => match screen.rows.get(*cursor).and_then(|r| r.action.clone()) {
                Some(RowAction::Jump(to)) => {
                    back.push(*cursor);
                    *cursor = to.min(last);
                    // The target goes to the top, so what follows it shows.
                    *top = *cursor;
                }
                Some(RowAction::Run(text)) if key.code == KeyCode::Enter => {
                    self.dialog = None;
                    return self.submit(text);
                }
                Some(RowAction::Confirm { question, command }) if key.code == KeyCode::Enter => {
                    let choices = vec![
                        Choice {
                            label: "Yes".into(),
                            detail: command.clone(),
                            pick: Pick::Run(command),
                            current: false,
                        },
                        Choice {
                            label: "No".into(),
                            detail: String::new(),
                            pick: Pick::Edit(String::new()),
                            current: false,
                        },
                    ];
                    self.dialog = None;
                    self.open_picker(Picker { title: question, choices });
                    return vec![];
                }
                Some(RowAction::Form(form)) if key.code == KeyCode::Enter => {
                    self.dialog = Some(Dialog {
                        prompt: None,
                        reply: None,
                        kind: DialogKind::Form { form, at: 0 },
                        selected: 0,
                        typing: None,
                    });
                    return vec![];
                }
                _ => {}
            },
            _ => {}
        }
        // Keep the highlighted row in view.
        if *cursor < *top {
            *top = *cursor;
        } else if *cursor >= *top + page {
            *top = *cursor + 1 - page;
        }
        vec![]
    }

    fn form_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog { kind: DialogKind::Form { form, at }, .. }) = self.dialog.as_mut() else { return vec![] };
        let n = form.fields.len();
        match key.code {
            KeyCode::Esc => self.dialog = None,
            KeyCode::Enter => {
                let text = form.command();
                self.dialog = None;
                return self.submit(text);
            }
            KeyCode::Tab | KeyCode::Down => *at = (*at + 1) % n.max(1),
            KeyCode::BackTab | KeyCode::Up => *at = at.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            code => match form.fields.get_mut(*at).map(|f| &mut f.kind) {
                Some(FieldKind::Text(t)) => match code {
                    KeyCode::Backspace => {
                        t.pop();
                    }
                    KeyCode::Char('u') if ctrl => t.clear(),
                    KeyCode::Char(c) if !ctrl => t.push(c),
                    _ => {}
                },
                Some(FieldKind::Choice { options, at: i }) => match code {
                    KeyCode::Left => *i = i.checked_sub(1).unwrap_or(options.len().saturating_sub(1)),
                    KeyCode::Right | KeyCode::Char(' ') => *i = (*i + 1) % options.len().max(1),
                    _ => {}
                },
                Some(FieldKind::Multi { options, picked, at: i }) => match code {
                    KeyCode::Left => *i = i.checked_sub(1).unwrap_or(options.len().saturating_sub(1)),
                    KeyCode::Right => *i = (*i + 1) % options.len().max(1),
                    KeyCode::Char(' ') if *i < picked.len() => picked[*i] = !picked[*i],
                    _ => {}
                },
                None => {}
            },
        }
        vec![]
    }

    /// Esc on a dialog that the reply can't reach any more (the turn was interrupted).
    pub fn close_dialog_if_stale(&mut self) {
        if self.dialog.as_ref().is_some_and(|d| d.reply.as_ref().is_some_and(|r| r.is_closed())) {
            self.dialog = self.held.take();
            self.dirty = true;
        }
    }

    // ---- the `/` menu ----

    /// Commands matching what is typed, while it is a bare `/word`.
    pub fn menu(&self) -> Vec<&CommandInfo> {
        if self.menu_dismissed || self.dialog.is_some() {
            return vec![];
        }
        let text = self.editor.text();
        let Some(word) = text.strip_prefix('/') else { return vec![] };
        if word.contains(char::is_whitespace) {
            return vec![];
        }
        let word = word.to_lowercase();
        let mut starts: Vec<&CommandInfo> = self.commands.iter().filter(|c| c.name.starts_with(&word)).collect();
        let contains = self.commands.iter().filter(|c| !c.name.starts_with(&word) && c.name.contains(&word));
        starts.extend(contains);
        starts
    }

    /// Files matching the `@word` before the cursor: name matches first, then shorter paths.
    pub fn file_menu(&self) -> Vec<&str> {
        if self.menu_dismissed || self.dialog.is_some() || self.search.is_some() {
            return vec![];
        }
        let (_, word) = self.editor.word_before_cursor();
        let Some(q) = word.strip_prefix('@') else { return vec![] };
        let q = q.to_lowercase();
        let mut hits: Vec<(bool, usize, &str)> = self
            .files
            .iter()
            .filter(|f| f.to_lowercase().contains(&q))
            .map(|f| {
                let name = f.trim_end_matches('/').rsplit('/').next().unwrap_or(f).to_lowercase();
                (!name.starts_with(&q), f.len(), f.as_str())
            })
            .collect();
        hits.sort();
        hits.into_iter().take(FILE_MATCHES).map(|(_, _, f)| f).collect()
    }

    fn complete_file(&mut self, path: &str) {
        let (_, word) = self.editor.word_before_cursor();
        // A path with spaces is quoted, the way `@` mentions read it.
        let mention = if path.contains(char::is_whitespace) { format!("@\"{path}\" ") } else { format!("@{path} ") };
        self.editor.replace_back(word.chars().count(), &mention);
    }

    // ---- history search (Ctrl+R) ----

    /// The newest history entry at or before `from` that contains `query`.
    fn find_history(&self, query: &str, from: Option<usize>) -> Option<usize> {
        let h = self.editor.history();
        let end = from.map(|f| f + 1).unwrap_or(h.len()).min(h.len());
        (0..end).rev().find(|&i| h[i].contains(query))
    }

    fn search_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut s = self.search.take()?;
        match key.code {
            KeyCode::Char('r') if ctrl => {
                let older = s.hit.and_then(|h| h.checked_sub(1)).and_then(|o| self.find_history(&s.query, Some(o)));
                s.hit = older.or(s.hit);
            }
            // Esc or Ctrl+G: back to what was typed.
            KeyCode::Esc => {
                self.editor.set(&s.draft);
                return Some(vec![]);
            }
            KeyCode::Char('g') if ctrl => {
                self.editor.set(&s.draft);
                return Some(vec![]);
            }
            KeyCode::Backspace => {
                s.query.pop();
                s.hit = self.find_history(&s.query, None);
            }
            KeyCode::Char(c) if !ctrl => {
                s.query.push(c);
                s.hit = self.find_history(&s.query, None);
            }
            // Enter takes the match into the input box; any other key takes it and then acts.
            KeyCode::Enter => return Some(vec![]),
            _ => return None,
        }
        match s.hit {
            Some(h) => {
                let text = self.editor.history()[h].clone();
                self.editor.set(&text);
            }
            None => self.editor.set(&s.draft),
        }
        self.search = Some(s);
        Some(vec![])
    }

    // ---- keys ----

    pub fn on_term_event(&mut self, ev: Event) -> Vec<Action> {
        self.dirty = true;
        match ev {
            // Through the user's key bindings first (keybindings.json).
            Event::Key(k) if k.kind != KeyEventKind::Release => match self.keymap.translate(k) {
                Some(k) => self.on_key(k),
                None => vec![],
            },
            Event::Paste(text) => {
                if let Some(e) = self.dialog.as_mut().and_then(|d| d.typing.as_mut()) {
                    e.insert(&text);
                } else if self.dialog.is_none() {
                    self.editor.insert(&text);
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn set_hint(&mut self, text: &str) {
        self.hint = Some((text.to_string(), Instant::now()));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let now = Instant::now();
        // Ctrl-C: clear, interrupt, or (twice) exit; it works in dialogs too.
        if ctrl && key.code == KeyCode::Char('c') {
            self.search = None;
            if self.dialog.is_some() {
                self.answer(PermissionAnswer::Deny { message: "The user interrupted.".into(), interrupt: true });
                return vec![Action::Interrupt];
            }
            if !self.editor.is_empty() {
                self.editor.clear();
                return vec![];
            }
            if self.busy {
                return vec![Action::Interrupt];
            }
            if self.last_ctrl_c.is_some_and(|t| now.duration_since(t) < DOUBLE_PRESS) {
                return vec![Action::Exit];
            }
            self.last_ctrl_c = Some(now);
            self.set_hint("Press Ctrl-C again to exit");
            return vec![];
        }
        if self.dialog.is_some() {
            return self.dialog_key(key);
        }
        if self.search.is_some() {
            if let Some(a) = self.search_key(key) {
                return a;
            }
        }
        if ctrl && key.code == KeyCode::Char('r') {
            let draft = self.editor.text();
            self.search = Some(Search { query: String::new(), hit: None, draft });
            return vec![];
        }
        if ctrl && key.code == KeyCode::Char('d') && self.editor.is_empty() {
            // A running turn stops first, so the session can end cleanly.
            return if self.busy { vec![Action::Interrupt, Action::Exit] } else { vec![Action::Exit] };
        }
        let files = self.file_menu().len();
        if files > 0 {
            match key.code {
                KeyCode::Up => {
                    self.menu_selected = self.menu_selected.checked_sub(1).unwrap_or(files - 1);
                    return vec![];
                }
                KeyCode::Down => {
                    self.menu_selected = (self.menu_selected + 1) % files;
                    return vec![];
                }
                KeyCode::Tab | KeyCode::Enter if !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                    let path = self.file_menu()[self.menu_selected.min(files - 1)].to_string();
                    self.complete_file(&path);
                    self.menu_selected = 0;
                    return vec![];
                }
                KeyCode::Esc => {
                    self.menu_dismissed = true;
                    return vec![];
                }
                _ => {}
            }
        }
        if key.code == KeyCode::BackTab {
            let cur = MODES.iter().position(|m| m.as_str() == self.status.mode).unwrap_or(0);
            let next = MODES[(cur + 1) % MODES.len()];
            self.status.mode = next.as_str().to_string();
            return vec![Action::SetMode(next)];
        }
        if key.code == KeyCode::Char('?') && self.editor.is_empty() && !ctrl && !alt {
            self.reply_lines(&keys_text(&self.keymap), false);
            return vec![];
        }
        let menu_len = self.menu().len();
        match key.code {
            KeyCode::Esc => {
                if menu_len > 0 {
                    self.menu_dismissed = true;
                    return vec![];
                }
                if self.busy {
                    return vec![Action::Interrupt];
                }
                if self.editor.is_empty() && self.last_esc.is_some_and(|t| now.duration_since(t) < DOUBLE_PRESS) {
                    self.last_esc = None;
                    return self.submit("/rewind".into());
                }
                self.last_esc = Some(now);
                if !self.editor.is_empty() {
                    self.set_hint("Esc again on an empty prompt opens /rewind");
                }
                return vec![];
            }
            KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                self.editor.insert("\n")
            }
            KeyCode::Char('j') if ctrl => self.editor.insert("\n"),
            KeyCode::Enter => {
                if menu_len > 0 {
                    let c = self.menu()[self.menu_selected.min(menu_len - 1)].clone();
                    // A command that takes arguments waits for them; one that doesn't runs now.
                    let typed = self.editor.text();
                    if c.args.is_empty() || typed == format!("/{}", c.name) && c.args.starts_with('[') {
                        self.editor.set(&format!("/{}", c.name));
                    } else {
                        self.editor.set(&format!("/{} ", c.name));
                        return vec![];
                    }
                }
                let text = self.editor.text();
                if let Some(stripped) = text.strip_suffix('\\') {
                    self.editor.set(&format!("{stripped}\n"));
                    return vec![];
                }
                if text.trim().is_empty() {
                    return vec![];
                }
                let text = self.editor.take();
                return self.submit(text);
            }
            KeyCode::Tab if menu_len > 0 => {
                let c = self.menu()[self.menu_selected.min(menu_len - 1)].clone();
                self.editor.set(&format!("/{} ", c.name));
            }
            KeyCode::Up if menu_len > 0 => {
                self.menu_selected = self.menu_selected.checked_sub(1).unwrap_or(menu_len - 1);
                return vec![];
            }
            KeyCode::Down if menu_len > 0 => {
                self.menu_selected = (self.menu_selected + 1) % menu_len;
                return vec![];
            }
            KeyCode::Up => {
                self.editor.up();
            }
            KeyCode::Down => {
                self.editor.down();
            }
            KeyCode::Left if ctrl => self.editor.word_left(),
            KeyCode::Right if ctrl => self.editor.word_right(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Backspace if alt || ctrl => self.editor.delete_word(),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Char('a') if ctrl => self.editor.home(),
            KeyCode::Char('e') if ctrl => self.editor.end(),
            KeyCode::Char('w') if ctrl => self.editor.delete_word(),
            KeyCode::Char('u') if ctrl => self.editor.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.editor.kill_to_end(),
            KeyCode::Char('b') if alt => self.editor.word_left(),
            KeyCode::Char('f') if alt => self.editor.word_right(),
            KeyCode::Char(c) if !ctrl => self.editor.insert(&c.to_string()),
            _ => {}
        }
        // Typing changes the menu: show it again from the top.
        self.menu_dismissed = false;
        if self.menu().len() != menu_len || self.file_menu().len() != files {
            self.menu_selected = 0;
        }
        vec![]
    }

    /// `/color [name|default]`: the accent colour for this session.
    fn color_command(&mut self, arg: &str) -> (String, bool) {
        let names: Vec<&str> = super::text::ACCENTS.iter().map(|(n, _)| *n).collect();
        let list = format!("Colours: {}, default.", names.join(", "));
        match arg {
            "" => (format!("The accent colour marks prompts, answers and selections. {list} /color <name> sets it for this session."), false),
            "default" | "reset" => {
                self.theme.accent = None;
                ("Accent colour: Forge's own.".into(), false)
            }
            name => match super::text::ACCENTS.iter().find(|(n, _)| *n == name) {
                Some((n, c)) => {
                    self.theme.accent = Some(*c);
                    let note = if self.theme.color { "" } else { " (Colour is off, so it shows once colour is on.)" };
                    (format!("Accent colour: {n}, for this session.{note}"), false)
                }
                None => (format!("Unknown colour {name:?}. {list}"), true),
            },
        }
    }

    /// `/focus [on|off]`: keep tool calls out of the scrollback.
    fn focus_command(&mut self, arg: &str) -> (String, bool) {
        let on = match arg {
            "" => !self.focus,
            "on" => true,
            "off" => false,
            _ => return ("Usage: /focus [on|off]".into(), true),
        };
        self.focus = on;
        let text = if on {
            "Focus on: tool calls and their results stay out of the scrollback (the spinner still names the tool). /focus again turns it off."
        } else {
            "Focus off: tool calls show again."
        };
        (text.into(), false)
    }

    /// Send `text` as if typed (queued while a turn runs).
    pub fn submit(&mut self, text: String) -> Vec<Action> {
        self.menu_selected = 0;
        self.menu_dismissed = false;
        // The UI's own immediate commands answer at once, even while a turn runs.
        let local = match text.trim() {
            "/keybindings" => Some((keys_text(&self.keymap), false)),
            "/terminal-setup" => Some((terminal_setup_text(super::keyboard_protocol()), false)),
            t if t == "/color" || t.starts_with("/color ") => Some(self.color_command(t["/color".len()..].trim())),
            t if t == "/focus" || t.starts_with("/focus ") => Some(self.focus_command(t["/focus".len()..].trim())),
            _ => None,
        };
        if let Some((answer, is_error)) = local {
            self.echo_prompt(&text);
            self.reply_lines(&answer, is_error);
            return vec![];
        }
        if self.busy {
            // Immediate commands (/status, /usage, /btw ...) are answered from the session view
            // while the turn runs (C17); anything else waits for the turn to end.
            if forge_core::commands::immediate(&text, &forge_core::commands::Catalog::default()) {
                self.echo_prompt(&text);
                return vec![Action::Send(text)];
            }
            self.queued.push_back(text);
            return vec![];
        }
        vec![Action::Send(self.start(text))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::text::plain;
    use forge_types::{ApiMessage, MessageContent, Role, StopReason, Usage};

    fn app() -> App {
        let mut a = App::new(Theme { color: false, light: false, accent: None }, vec![]);
        a.commands = vec![
            CommandInfo { name: "clear".into(), args: "[name]".into(), description: "Start a new conversation".into() },
            CommandInfo { name: "compact".into(), args: "[instructions]".into(), description: "Summarize".into() },
            CommandInfo { name: "model".into(), args: "[model]".into(), description: "Switch models".into() },
            CommandInfo { name: "status".into(), args: String::new(), description: "Show status".into() },
        ];
        a
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(a: &mut App, s: &str) {
        for c in s.chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(plain).collect()
    }

    fn delta(text: &str) -> UiEvent {
        UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockDelta { index: 0, delta: Delta::TextDelta { text: text.into() } },
            parent_tool_use_id: None,
        })
    }

    #[test]
    fn enter_sends_and_queues_while_busy() {
        let mut a = app();
        typed(&mut a, "hello");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("hello".into())]);
        assert!(a.busy);
        assert_eq!(texts(&a.take_pending()), ["", "> hello "]);
        typed(&mut a, "next");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "queued while a turn runs");
        assert_eq!(a.queued.len(), 1);
        assert_eq!(a.next_queued(), None, "still busy");
        a.on_event(UiEvent::Idle);
        assert_eq!(a.next_queued().as_deref(), Some("next"));
        // A trailing backslash, Shift+Enter or Ctrl+J start a new line instead.
        a.on_event(UiEvent::Idle);
        typed(&mut a, "one\\");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        typed(&mut a, "two");
        a.on_key(ctrl('j'));
        assert_eq!(a.editor.text(), "one\ntwo\n");
    }

    #[test]
    fn key_bindings_apply_and_show_in_the_key_table() {
        let mut a = app();
        let (k, w) = crate::tui::keys::Keymap::parse(r#"{"ctrl+s": "submit", "ctrl+r": "none"}"#);
        assert!(w.is_empty(), "{w:?}");
        a.keymap = k;
        let term = |a: &mut App, code, m| a.on_term_event(Event::Key(KeyEvent::new(code, m)));
        typed(&mut a, "hello");
        assert_eq!(term(&mut a, KeyCode::Char('s'), KeyModifiers::CONTROL), vec![Action::Send("hello".into())]);
        a.on_event(UiEvent::Idle);
        term(&mut a, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(a.search.is_none(), "Ctrl+R is unbound");
        let table = keys_text(&a.keymap);
        assert!(table.contains("enter, ctrl+s") && table.contains("Unbound: ctrl+r."), "{table}");
        assert!(keys_text(&Default::default()).starts_with("Enter "), "the defaults read as before");
    }

    #[test]
    fn color_and_focus_change_this_session_only() {
        let mut a = App::new(Theme { color: true, light: false, accent: None }, vec![]);
        assert!(a.submit("/color green".into()).is_empty(), "answered by the UI");
        assert_eq!(a.theme.accent, Some(ratatui::style::Color::Green));
        assert_eq!(a.theme.accent().fg, Some(ratatui::style::Color::Green));
        a.on_event(UiEvent::Theme("light".into()));
        assert_eq!(a.theme.accent, Some(ratatui::style::Color::Green), "a theme change keeps it");
        a.take_pending();
        a.submit("/color mauve".into());
        assert!(texts(&a.take_pending()).iter().any(|l| l.contains("Unknown colour \"mauve\"")));
        a.submit("/color default".into());
        assert_eq!(a.theme.accent, None);

        // Focus: tool calls and results stay out of the scrollback; text still shows.
        a.submit("/focus".into());
        assert!(a.focus);
        a.take_pending();
        a.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: Role::Assistant,
                model: "m".into(),
                content: vec![
                    ContentBlock::text("Looking."),
                    ContentBlock::ToolUse {
                        id: "t".into(),
                        name: "Bash".into(),
                        input: json!({"command": "ls"}),
                        cache_control: None,
                    },
                ],
                stop_reason: Some(StopReason::ToolUse),
                stop_sequence: None,
                usage: Usage::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        a.on_event(UiEvent::Engine(EngineEvent::User {
            message: forge_types::Message::user(vec![ContentBlock::tool_result("t".to_string(), "file.txt", false)]),
            uuid: "u2".into(),
            tool_use_result: None,
            is_meta: false,
            parent_tool_use_id: None,
        }));
        let lines = texts(&a.take_pending());
        assert!(lines.iter().any(|l| l.contains("Looking.")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("Bash") || l.contains("file.txt")), "{lines:?}");
        assert_eq!(a.activity, "Working");
        a.submit("/focus off".into());
        assert!(!a.focus);
    }

    #[test]
    fn a_screen_arriving_during_a_question_waits_for_its_answer() {
        let mut a = app();
        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask {
            prompt: PermissionPrompt {
                tool_name: "Bash".into(),
                tool_use_id: "t".into(),
                input: json!({"command": "ls"}),
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            },
            reply: tx,
        });
        // /context typed mid-turn comes back as a screen while the question is open.
        let screen = forge_core::commands::screens::Screen { title: "Context".into(), rows: vec![] };
        a.on_event(UiEvent::Screen(screen));
        assert_eq!(a.dialog_text().0, "Allow Bash?", "the question stays");
        assert!(rx.try_recv().is_err(), "and isn't answered");
        a.on_key(key(KeyCode::Char('1')));
        assert!(matches!(rx.try_recv(), Ok(PermissionAnswer::Allow { .. })));
        assert_eq!(a.viewer().map(|v| v.0.title.as_str()), Some("Context"), "then the screen shows");
    }

    #[test]
    fn immediate_commands_are_sent_while_busy() {
        let mut a = app();
        typed(&mut a, "hello");
        a.on_key(key(KeyCode::Enter));
        a.take_pending();
        typed(&mut a, "/usage");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/usage".into())], "sent, not queued");
        assert!(a.queued.is_empty() && a.busy, "the turn goes on");
        assert_eq!(texts(&a.take_pending()), ["", "> /usage "], "echoed");
        typed(&mut a, "/compact");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "not immediate: queued");
        assert_eq!(a.queued.len(), 1);
    }

    #[test]
    fn streamed_text_moves_to_scrollback_line_by_line() {
        let mut a = app();
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlock::text("") },
            parent_tool_use_id: None,
        }));
        a.on_event(delta("Here is **the** plan:\n- step"));
        assert_eq!(texts(&a.pending), ["", "• Here is the plan:"]);
        assert_eq!(a.live, "- step");
        a.on_event(delta(" one\n```sh\nls\n"));
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockStop { index: 0 },
            parent_tool_use_id: None,
        }));
        assert_eq!(texts(&a.take_pending())[2..], ["  - step one", "  ```sh", "    ls"]);
        // The complete message adds its tool call, not its text again.
        let msg = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![
                ContentBlock::text("Here is the plan"),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Bash".into(),
                    input: json!({"command": "cargo test"}),
                    cache_control: None,
                },
            ],
            stop_reason: Some(StopReason::ToolUse),
            stop_sequence: None,
            usage: Usage::default(),
        };
        a.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: msg,
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        assert_eq!(texts(&a.take_pending()), ["", "› Bash(cargo test)"]);
        a.on_event(UiEvent::Engine(EngineEvent::User {
            message: forge_types::Message::user(vec![ContentBlock::tool_result("t1", "ok\n2 passed", false)]),
            uuid: "u2".into(),
            tool_use_result: None,
            is_meta: false,
            parent_tool_use_id: None,
        }));
        assert_eq!(texts(&a.take_pending()), ["  ↳  ok (+1 lines)"]);
        let _ = MessageContent::Text(String::new());
    }

    #[test]
    fn slash_menu_filters_completes_and_runs() {
        let mut a = app();
        typed(&mut a, "/c");
        let names: Vec<&str> = a.menu().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["clear", "compact"]);
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "/compact ");
        assert!(a.menu().is_empty(), "the menu closes after the name");
        a.editor.clear();
        typed(&mut a, "/stat");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/status".into())]);
        a.on_event(UiEvent::Idle);
        typed(&mut a, "/mo");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "a command with arguments waits for them");
        assert_eq!(a.editor.text(), "/model ");
        a.editor.clear();
        typed(&mut a, "/zz");
        assert!(a.menu().is_empty());
        typed(&mut a, "\u{8}");
        a.editor.clear();
        typed(&mut a, "/cl");
        a.on_key(key(KeyCode::Esc));
        assert!(a.menu().is_empty(), "Esc hides the menu");
    }

    #[test]
    fn esc_ctrl_c_and_shift_tab() {
        let mut a = app();
        a.status.mode = "default".into();
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            vec![Action::SetMode(PermissionMode::AcceptEdits)]
        );
        assert_eq!(a.status.mode, "acceptEdits");
        typed(&mut a, "x");
        a.on_key(ctrl('c'));
        assert!(a.editor.is_empty(), "Ctrl-C clears first");
        assert!(a.on_key(ctrl('c')).is_empty());
        assert_eq!(a.on_key(ctrl('c')), vec![Action::Exit], "twice exits");
        a.busy = true;
        assert_eq!(a.on_key(key(KeyCode::Esc)), vec![Action::Interrupt]);
        a.busy = false;
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.on_key(key(KeyCode::Esc)), vec![Action::Send("/rewind".into())]);
        // Ctrl-D during a turn stops it before leaving.
        assert_eq!(a.on_key(ctrl('d')), vec![Action::Interrupt, Action::Exit]);
    }

    fn prompt(tool: &str, input: Value) -> PermissionPrompt {
        PermissionPrompt {
            tool_name: tool.into(),
            tool_use_id: "t".into(),
            input,
            reason: String::new(),
            suggestions: vec![],
            blocked_path: None,
        }
    }

    #[test]
    fn dialogs_answer_permissions_questions_and_plans() {
        let mut a = app();
        let (tx, mut rx) = oneshot::channel();
        let mut p = prompt("Bash", json!({"command": "npm test"}));
        p.suggestions = vec![Suggestion::AddRules {
            rules: vec![serde_json::from_value(json!({"toolName": "Bash", "ruleContent": "npm test"})).unwrap()],
            behavior: forge_permissions::Behavior::Allow,
            destination: "localSettings".into(),
        }];
        a.on_event(UiEvent::Ask { prompt: p, reply: tx });
        assert_eq!(a.dialog_text().0, "Allow Bash?");
        assert_eq!(a.dialog_options()[1].0, "Yes, and don't ask again for Bash(npm test)");
        a.on_key(key(KeyCode::Char('2')));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_permissions, .. } => assert_eq!(updated_permissions.len(), 1),
            other => panic!("{other:?}"),
        }
        assert!(a.dialog.is_none());

        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask { prompt: prompt("Write", json!({"file_path": "a.txt"})), reply: tx });
        a.on_key(key(KeyCode::Esc));
        assert!(matches!(rx.try_recv().unwrap(), PermissionAnswer::Deny { interrupt: true, .. }));

        // Two questions: a pick, then a typed answer.
        let (tx, mut rx) = oneshot::channel();
        let q = json!({"questions": [
            {"question": "Which db?", "options": [{"label": "sqlite"}, {"label": "postgres"}]},
            {"question": "Name?", "options": [{"label": "app"}]}
        ]});
        a.on_event(UiEvent::Ask { prompt: prompt("AskUserQuestion", q), reply: tx });
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.dialog_text().0, "Name? (2/2)");
        a.on_key(key(KeyCode::Char('2')));
        typed(&mut a, "forge");
        a.on_key(key(KeyCode::Enter));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_input: Some(v), .. } => {
                assert_eq!(v["answers"], json!({"Which db?": "postgres", "Name?": "forge"}))
            }
            other => panic!("{other:?}"),
        }

        // A plan: written to scrollback; "Yes, and accept edits" switches the mode.
        let (tx, mut rx) = oneshot::channel();
        a.take_pending();
        a.on_event(UiEvent::Ask { prompt: prompt("ExitPlanMode", json!({"plan": "1. Edit a\n2. Test"})), reply: tx });
        assert_eq!(texts(&a.take_pending())[1..], ["Here is the plan:", "  1. Edit a", "  2. Test"]);
        a.on_key(key(KeyCode::Enter));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_permissions, .. } => {
                assert_eq!(updated_permissions[0]["mode"], "acceptEdits")
            }
            other => panic!("{other:?}"),
        }
    }

    fn picker() -> Picker {
        use forge_core::commands::picker::Choice;
        let c = |label: &str, pick: Pick, current: bool| Choice {
            label: label.into(),
            detail: String::new(),
            pick,
            current,
        };
        Picker {
            title: "Pick".into(),
            choices: vec![
                c("Opus", Pick::Run("/model opus".into()), false),
                c("Haiku", Pick::Run("/model haiku".into()), true),
                c("Step", Pick::Step("/rewind 1".into()), false),
                c("Back", Pick::RunThenEdit { command: "/rewind 1 both".into(), edit: "old prompt".into() }, false),
                c("Add", Pick::Edit("/permissions add allow ".into()), false),
            ],
        }
    }

    #[test]
    fn pickers_filter_and_run_command_text() {
        let mut a = app();
        a.on_event(UiEvent::Picker(picker()));
        assert_eq!(a.dialog_selected(), 1, "the current choice is selected");
        assert_eq!(a.dialog_options()[1].0, "Haiku ✔");
        typed(&mut a, "op");
        assert_eq!(a.dialog_options().len(), 1);
        assert_eq!(a.dialog_text().1, ["Filter: op"]);
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/model opus".into())]);
        assert!(a.dialog.is_none());
        a.on_event(UiEvent::Idle);

        a.on_event(UiEvent::Picker(picker()));
        typed(&mut a, "step");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Picker("/rewind 1".into())]);
        a.on_event(UiEvent::Picker(picker()));
        typed(&mut a, "back");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/rewind 1 both".into())]);
        assert_eq!(a.editor.text(), "old prompt", "the rewound prompt is back in the input box");
        a.on_event(UiEvent::Idle);
        a.editor.clear();
        a.on_event(UiEvent::Picker(picker()));
        typed(&mut a, "add");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(a.editor.text(), "/permissions add allow ");
        a.on_event(UiEvent::Picker(picker()));
        a.on_key(key(KeyCode::Esc));
        assert!(a.dialog.is_none(), "Esc closes a picker");
    }

    #[test]
    fn ctrl_r_searches_history() {
        let mut a = App::new(
            Theme { color: false, light: false, accent: None },
            vec!["cargo test".into(), "git status".into(), "cargo build".into()],
        );
        typed(&mut a, "draft");
        a.on_key(ctrl('r'));
        typed(&mut a, "cargo");
        assert_eq!(a.editor.text(), "cargo build", "the newest match first");
        a.on_key(ctrl('r'));
        assert_eq!(a.editor.text(), "cargo test", "Ctrl+R again: older");
        a.on_key(ctrl('r'));
        assert_eq!(a.editor.text(), "cargo test", "no older match: it stays");
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.editor.text(), "draft", "Esc goes back to what was typed");
        assert!(a.search.is_none());
        a.on_key(ctrl('r'));
        typed(&mut a, "stat");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "Enter takes the match, it doesn't send");
        assert_eq!(a.editor.text(), "git status");
        assert!(a.search.is_none());
        // Another key takes the match and acts.
        a.editor.clear();
        a.on_key(ctrl('r'));
        typed(&mut a, "build");
        a.on_key(key(KeyCode::End));
        typed(&mut a, " --release");
        assert_eq!(a.editor.text(), "cargo build --release");
    }

    #[test]
    fn at_completes_paths() {
        let mut a = app();
        a.on_event(UiEvent::Files(vec![
            "src/".into(),
            "src/main.rs".into(),
            "docs/main-notes.md".into(),
            "Cargo.toml".into(),
            "my notes.txt".into(),
        ]));
        typed(&mut a, "explain @mai");
        assert_eq!(a.file_menu(), ["src/main.rs", "docs/main-notes.md"], "name matches, shorter first");
        a.on_key(key(KeyCode::Down));
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "Enter completes instead of sending");
        assert_eq!(a.editor.text(), "explain @docs/main-notes.md ");
        assert!(a.file_menu().is_empty());
        typed(&mut a, "and @CARGO");
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "explain @docs/main-notes.md and @Cargo.toml ");
        typed(&mut a, "@src");
        a.on_key(key(KeyCode::Esc));
        assert!(a.file_menu().is_empty(), "Esc hides it");
        assert_eq!(
            a.on_key(key(KeyCode::Enter)),
            vec![Action::Send("explain @docs/main-notes.md and @Cargo.toml @src".into())]
        );
        a.busy = false;
        typed(&mut a, "@my");
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "@\"my notes.txt\" ", "spaces are quoted");
    }

    #[test]
    fn attached_files_show_under_the_prompt() {
        let mut a = app();
        let reminder = forge_agents::attach::reminder(&[forge_agents::attach::Attachment {
            path: "/w/src/a.rs".into(),
            kind: forge_agents::attach::Kind::File,
            text: "fn a() {}".into(),
        }])
        .unwrap();
        a.on_event(UiEvent::Engine(EngineEvent::PromptAccepted {
            message: forge_types::Message::user(vec![
                ContentBlock::text("explain @src/a.rs"),
                ContentBlock::text(reminder),
            ]),
            uuid: "u".into(),
        }));
        assert_eq!(texts(&a.pending), ["  (attached: a.rs)"]);
    }

    #[test]
    fn question_mark_themes_and_clipboard() {
        let mut a = App::new(Theme { color: true, light: false, accent: None }, vec![]);
        a.on_key(key(KeyCode::Char('?')));
        let shown = texts(&a.take_pending()).join("\n");
        assert!(shown.contains("Shift+Tab") && shown.contains("Ctrl+R"), "{shown}");
        assert!(a.editor.is_empty(), "? on an empty prompt isn't typed");
        typed(&mut a, "why?");
        assert_eq!(a.editor.text(), "why?");

        a.on_event(UiEvent::Theme("light".into()));
        assert_eq!(a.theme, Theme { color: true, light: true, accent: None });
        a.on_event(UiEvent::Theme("none".into()));
        assert!(!a.theme.color);
        a.on_event(UiEvent::Theme("dark".into()));
        assert_eq!(a.theme, Theme { color: true, light: false, accent: None });
        // Without colour allowed, no theme brings it back.
        let mut b = App::new(Theme { color: false, light: false, accent: None }, vec![]);
        b.on_event(UiEvent::Theme("light".into()));
        assert!(!b.theme.color);

        a.on_event(UiEvent::Copy("text".into()));
        assert_eq!(a.clipboard.as_deref(), Some("text"));
        a.on_event(UiEvent::StatusLine(Some("custom".into())));
        assert_eq!(a.status_text.as_deref(), Some("custom"));
    }

    #[test]
    fn ui_commands_answer_at_once_while_busy() {
        let mut a = app();
        typed(&mut a, "long task");
        a.on_key(key(KeyCode::Enter));
        a.take_pending();
        typed(&mut a, "/keybindings");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(a.queued.is_empty(), "not queued behind the turn");
        assert!(texts(&a.take_pending()).iter().any(|l| l.contains("Shift+Tab")));
        assert!(terminal_setup_text(false).contains("Alt+Enter") && terminal_setup_text(true).contains("works"));
    }
}
