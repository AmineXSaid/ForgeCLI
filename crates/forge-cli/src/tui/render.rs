//! The live region at the bottom of the terminal, drawn from the app state:
//! the unfinished answer line, the spinner, a dialog, queued messages, the
//! input box, the `/` menu and the status line (docs/TUI.md, "Renderer").

use std::time::{Duration, Instant};

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::app::App;
use super::text::{self, width as text_width};

/// The rows to draw, and where the cursor goes (column, row) when it shows.
#[derive(Debug, Clone, Default)]
pub struct LiveView {
    pub lines: Vec<Line<'static>>,
    pub cursor: Option<(u16, u16)>,
}

const SPINNER: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];
pub const SPINNER_STEP: Duration = Duration::from_millis(120);
/// How long a hint stays in the status line.
pub const HINT_FOR: Duration = Duration::from_secs(2);
const MENU_ROWS: usize = 8;
const LIVE_ROWS: usize = 3;
pub const PLACEHOLDER: &str = "Try \"explain this repo\" · / for commands";

/// `s` cut to `w` columns, with an ellipsis when it was longer.
pub fn fit(s: &str, w: usize) -> String {
    if text_width(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    if w > 0 {
        out.push('…');
    }
    out
}

/// Split one line of text into rows of at most `w` columns, by character.
/// Returns the rows' character ranges.
fn char_rows(chars: &[char], w: usize) -> Vec<(usize, usize)> {
    let w = w.max(1);
    let mut rows = vec![];
    let mut start = 0;
    let mut used = 0;
    for (i, c) in chars.iter().enumerate() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w && i > start {
            rows.push((start, i));
            start = i;
            used = 0;
        }
        used += cw;
    }
    rows.push((start, chars.len()));
    rows
}

fn status_left(app: &App, now: Instant) -> (String, Style) {
    if let Some((h, at)) = &app.hint {
        if now.duration_since(*at) < HINT_FOR {
            return (h.clone(), app.theme.dim());
        }
    }
    let t = app.theme;
    match app.status.mode.as_str() {
        "acceptEdits" => ("⏵⏵ accept edits on (shift+tab to cycle)".into(), t.warning()),
        "plan" => ("⏸ plan mode on (shift+tab to cycle)".into(), t.success()),
        "bypassPermissions" => ("⏵⏵ bypass permissions on".into(), t.error()),
        "dontAsk" => ("don't ask mode on (shift+tab to cycle)".into(), t.dim()),
        _ => ("? for shortcuts".into(), t.dim()),
    }
}

fn status_right(app: &App) -> String {
    let s = &app.status;
    let mut parts = vec![];
    if !s.model.is_empty() {
        parts.push(s.model.clone());
    }
    if let Some(p) = s.context_pct {
        parts.push(format!("{p}% context"));
    }
    parts.push(format!("${:.2}", s.cost));
    parts.join(" · ")
}

/// A rounded box around `body` rows, `w` columns wide.
fn boxed(title: Line<'static>, body: Vec<Line<'static>>, w: usize, border: Style) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4).max(1);
    let mut out = vec![Line::from(Span::styled(format!("╭{}╮", "─".repeat(w.saturating_sub(2))), border))];
    for l in std::iter::once(title).chain(body) {
        for row in text::wrap(vec![l], inner as u16) {
            let used: usize = row.spans.iter().map(|s| text_width(&s.content)).sum();
            let mut spans = vec![Span::styled("│ ", border)];
            spans.extend(row.spans);
            spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
            spans.push(Span::styled(" │", border));
            out.push(Line::from(spans));
        }
    }
    out.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border)));
    out
}

fn dialog(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let (title, body) = app.dialog_text();
    let mut rows: Vec<Line<'static>> = body.into_iter().map(Line::from).collect();
    rows.push(Line::default());
    let selected = app.dialog_selected();
    for (i, (label, desc)) in app.dialog_options().into_iter().enumerate() {
        let mark = if i == selected { "❯ " } else { "  " };
        let st = if i == selected { t.selected() } else { Style::default() };
        let mut spans = vec![Span::styled(format!("{mark}{}. {label}", i + 1), st)];
        if !desc.is_empty() {
            spans.push(Span::styled(format!("  {desc}"), t.dim()));
        }
        rows.push(Line::from(spans));
    }
    rows.push(Line::default());
    rows.push(Line::from(Span::styled("Enter to select · Esc to cancel", t.dim())));
    boxed(Line::from(Span::styled(title, t.bold())), rows, w, t.accent())
}

/// The input box's rows and the cursor's (column, row) inside them.
fn input(app: &App, w: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let t = app.theme;
    let text_w = w.saturating_sub(2).max(1);
    if app.editor.is_empty() && !app.busy {
        let row = Line::from(vec![Span::styled("> ", t.accent()), Span::styled(fit(PLACEHOLDER, text_w), t.dim())]);
        return (vec![row], (2, 0));
    }
    let (cur_line, cur_col) = app.editor.position();
    let mut rows = vec![];
    let mut cursor = (2u16, 0u16);
    for (li, line) in app.editor.text().split('\n').enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let ranges = char_rows(&chars, text_w);
        for (ri, (a, b)) in ranges.iter().enumerate() {
            if li == cur_line {
                // The cursor sits in this row, or at the end of the last one.
                let last = ri + 1 == ranges.len();
                if cur_col >= *a && (cur_col < *b || last) {
                    let col: usize = chars[*a..cur_col].iter().map(|c| c.width().unwrap_or(0)).sum();
                    cursor = ((2 + col).min(w.saturating_sub(1)) as u16, rows.len() as u16);
                }
            }
            let prefix = if rows.is_empty() { Span::styled("> ", t.accent()) } else { Span::raw("  ") };
            rows.push(Line::from(vec![prefix, Span::raw(chars[*a..*b].iter().collect::<String>())]));
        }
    }
    (rows, cursor)
}

/// The `@` menu: matching paths.
fn file_menu(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let items = app.file_menu();
    if items.is_empty() {
        return vec![];
    }
    let sel = app.menu_selected.min(items.len() - 1);
    let first = sel.saturating_sub(MENU_ROWS - 1);
    items[first..items.len().min(first + MENU_ROWS)]
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let st = if first + i == sel { t.selected() } else { Style::default() };
            Line::from(Span::styled(fit(&format!("  + {f}"), w), st))
        })
        .collect()
}

fn menu(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let items = app.menu();
    if items.is_empty() {
        return file_menu(app, w);
    }
    let sel = app.menu_selected.min(items.len() - 1);
    let first = sel.saturating_sub(MENU_ROWS - 1);
    let shown = &items[first..items.len().min(first + MENU_ROWS)];
    let label = |c: &super::app::CommandInfo| {
        if c.args.is_empty() {
            format!("/{}", c.name)
        } else {
            format!("/{} {}", c.name, c.args)
        }
    };
    let pad = shown.iter().map(|c| text_width(&label(c))).max().unwrap_or(0).min(w / 2);
    shown
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let l = fit(&label(c), pad);
            let l = format!("  {l}{}", " ".repeat(pad.saturating_sub(text_width(&l))));
            let desc = fit(&c.description, w.saturating_sub(text_width(&l) + 2));
            let selected = first + i == sel;
            let st = if selected { t.selected() } else { Style::default() };
            Line::from(vec![Span::styled(l, st), Span::raw("  "), Span::styled(desc, t.dim())])
        })
        .collect()
}

fn status(app: &App, w: usize, now: Instant) -> Line<'static> {
    let (left, left_style) = status_left(app, now);
    let right = status_right(app);
    let left = fit(&left, w.saturating_sub(2));
    let room = w.saturating_sub(text_width(&left) + 4);
    let right = if room >= 8 { fit(&right, room) } else { String::new() };
    let gap = w.saturating_sub(2 + text_width(&left) + text_width(&right));
    Line::from(vec![
        Span::raw("  "),
        Span::styled(left, left_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(right, app.theme.dim()),
    ])
}

/// The live region for a terminal `width` columns wide, at most `max_height` rows.
pub fn live_view(app: &App, width: u16, max_height: u16) -> LiveView {
    live_view_at(app, width, max_height, Instant::now())
}

pub fn live_view_at(app: &App, width: u16, max_height: u16, now: Instant) -> LiveView {
    let w = (width as usize).max(4);
    let t = app.theme;
    // 1. The unfinished answer line: its last rows.
    let mut live: Vec<Line<'static>> = app.live_line().map(|l| text::wrap(vec![l], w as u16)).unwrap_or_default();
    if live.len() > LIVE_ROWS {
        live.drain(..live.len() - LIVE_ROWS);
    }
    // 2. The spinner.
    let mut spinner = vec![];
    if app.busy {
        let since = app.busy_since.map(|s| now.duration_since(s)).unwrap_or_default();
        let frame = SPINNER[(since.as_millis() / SPINNER_STEP.as_millis()) as usize % SPINNER.len()];
        let activity = if app.activity.is_empty() { "Working" } else { app.activity.as_str() };
        let text = format!("{frame} {activity}… ({}s · esc to interrupt)", since.as_secs());
        spinner.push(Line::default());
        spinner.push(Line::from(Span::styled(fit(&text, w), t.accent())));
    }
    // 3. A dialog.
    let dialog = if app.dialog.is_some() { dialog(app, w) } else { vec![] };
    // 4. Queued messages.
    let mut queued: Vec<Line<'static>> = app
        .queued
        .iter()
        .map(|q| Line::from(Span::styled(fit(&format!("  ⏎ {}", q.replace('\n', " ")), w), t.dim())))
        .collect();
    // 5. The input box (hidden while a dialog is open).
    let rule = Line::from(Span::styled("─".repeat(w), t.dim()));
    let (input_rows, cursor) = if app.dialog.is_none() { input(app, w) } else { (vec![], (0, 0)) };
    // Ctrl+R: what is being searched for, above the input.
    let search: Vec<Line<'static>> = app
        .search
        .as_ref()
        .map(|s| {
            let miss = if s.hit.is_none() && !s.query.is_empty() { " (no match)" } else { "" };
            Line::from(Span::styled(fit(&format!("  search history: {}{miss}", s.query), w), t.accent()))
        })
        .into_iter()
        .collect();
    // 6. The `/` or `@` menu, 7. the status line.
    let menu = menu(app, w);
    let status = status(app, w, now);

    let fixed = spinner.len()
        + search.len()
        + dialog.len()
        + if input_rows.is_empty() { 0 } else { input_rows.len() + 2 }
        + menu.len()
        + 1;
    let max = (max_height as usize).max(1);
    let spare = max.saturating_sub(fixed);
    // Too tall: drop the answer's rows first, then queued messages.
    while live.len() + queued.len() > spare {
        if !live.is_empty() {
            live.remove(0);
        } else {
            queued.remove(0);
        }
    }

    let mut lines = live;
    lines.extend(spinner);
    lines.extend(dialog);
    lines.extend(queued);
    lines.extend(search);
    let mut cur = None;
    if !input_rows.is_empty() {
        lines.push(rule.clone());
        cur = Some((cursor.0, (lines.len() as u16) + cursor.1));
        lines.extend(input_rows);
        lines.push(rule);
    }
    lines.extend(menu);
    lines.push(status);
    // The input itself can be taller than the screen: keep the rows around the cursor.
    if lines.len() > max {
        let cut = lines.len() - max;
        let keep_from = cur.map(|(_, y)| (y as usize).min(cut)).unwrap_or(cut);
        lines.drain(..keep_from);
        cur = cur.map(|(x, y)| (x, y - keep_from as u16));
        lines.truncate(max);
        cur = cur.filter(|(_, y)| (*y as usize) < lines.len());
    }
    LiveView { lines, cursor: cur }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{CommandInfo, StatusView, UiEvent};
    use crate::tui::text::Theme;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use forge_engine::PermissionPrompt;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;
    use serde_json::json;

    fn app(color: bool) -> App {
        let mut a = App::new(Theme { color }, vec![]);
        a.on_event(UiEvent::Status(StatusView {
            model: "opus".into(),
            mode: "default".into(),
            cwd: "/p".into(),
            cost: 0.25,
            context_pct: Some(12),
        }));
        a.on_event(UiEvent::Commands(vec![
            CommandInfo { name: "clear".into(), args: "[name]".into(), description: "Start a new conversation".into() },
            CommandInfo { name: "compact".into(), args: "[instructions]".into(), description: "Summarize".into() },
            CommandInfo { name: "status".into(), args: String::new(), description: "Show status".into() },
        ]));
        a
    }

    /// Draw the view into a test terminal; return its rows (trailing spaces cut).
    fn draw(view: &LiveView, w: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
        let h = view.lines.len() as u16;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| f.render_widget(Paragraph::new(view.lines.clone()), f.area())).unwrap();
        let buf = term.backend().buffer().clone();
        let rows = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
            .collect();
        (rows, buf)
    }

    fn typed(a: &mut App, s: &str) {
        for c in s.chars() {
            a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn idle_shows_the_placeholder_and_status() {
        let a = app(true);
        let v = live_view(&a, 60, 20);
        let (rows, buf) = draw(&v, 60);
        assert_eq!(rows[0], "─".repeat(60));
        assert_eq!(rows[1], format!("> {PLACEHOLDER}"));
        assert_eq!(rows[2], "─".repeat(60));
        assert_eq!(rows[3], "  ? for shortcuts                 opus · 12% context · $0.25");
        assert_eq!(v.cursor, Some((2, 1)));
        // The placeholder is dim; the prompt marker uses the accent colour.
        assert!(buf[(4, 1)].modifier.contains(ratatui::style::Modifier::DIM));
        assert_eq!(buf[(0, 1)].fg, ratatui::style::Color::Rgb(0x9b, 0x7b, 0xf0));
    }

    #[test]
    fn busy_shows_the_answer_line_spinner_and_queue() {
        let mut a = app(true);
        typed(&mut a, "hello");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let start = a.busy_since.unwrap();
        a.live = "partial **answer**".into();
        typed(&mut a, "next");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        typed(&mut a, "ab");
        let v = live_view_at(&a, 50, 20, start + Duration::from_millis(1300));
        let (rows, _) = draw(&v, 50);
        assert_eq!(rows[0], "⏺ partial answer");
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "✻ Working… (1s · esc to interrupt)", "1.3 s is frame 10, the 5th of 6");
        assert_eq!(rows[3], "  ⏎ next");
        assert_eq!(rows[5], "> ab");
        assert_eq!(v.cursor, Some((4, 5)));
        // Too short a screen drops the answer line first, then queued messages, never the input.
        let v = live_view_at(&a, 50, 6, start);
        let (rows, _) = draw(&v, 50);
        assert!(rows.iter().all(|r| !r.contains("partial")), "{rows:?}");
        assert!(rows.iter().any(|r| r == "> ab"), "{rows:?}");
        assert_eq!(v.lines.len(), 6);
    }

    #[test]
    fn the_menu_lists_matching_commands() {
        let mut a = app(true);
        typed(&mut a, "/c");
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let v = live_view(&a, 60, 20);
        let (rows, buf) = draw(&v, 60);
        assert_eq!(rows[1], "> /c");
        assert_eq!(rows[3], "  /clear [name]            Start a new conversation");
        assert_eq!(rows[4], "  /compact [instructions]  Summarize");
        // The selected row is highlighted.
        assert!(buf[(3, 4)].modifier.contains(ratatui::style::Modifier::BOLD));
        assert!(!buf[(3, 3)].modifier.contains(ratatui::style::Modifier::BOLD));
    }

    fn ask(a: &mut App, tool: &str, input: serde_json::Value) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::mem::forget(rx);
        a.on_event(UiEvent::Ask {
            prompt: PermissionPrompt {
                tool_name: tool.into(),
                tool_use_id: "t".into(),
                input,
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            },
            reply: tx,
        });
    }

    #[test]
    fn dialogs_draw_in_a_box_without_the_input() {
        let mut a = app(false);
        ask(&mut a, "Bash", json!({"command": "rm -rf build"}));
        let v = live_view(&a, 40, 30);
        let (rows, buf) = draw(&v, 40);
        assert_eq!(
            rows,
            [
                "╭──────────────────────────────────────╮",
                "│ Allow Bash?                          │",
                "│   rm -rf build                       │",
                "│                                      │",
                "│ ❯ 1. Yes                             │",
                "│   2. No, and tell Forge what to do   │",
                "│ instead  esc                         │",
                "│                                      │",
                "│ Enter to select · Esc to cancel      │",
                "╰──────────────────────────────────────╯",
                "  ? for shortcuts  opus · 12% context ·…",
            ]
        );
        assert_eq!(v.cursor, None, "no cursor while a dialog is open");
        // Without colour the selection is reversed.
        assert!(buf[(4, 4)].modifier.contains(ratatui::style::Modifier::REVERSED));

        let mut a = app(false);
        ask(&mut a, "ExitPlanMode", json!({"plan": "1. Do it"}));
        let (rows, _) = draw(&live_view(&a, 50, 30), 50);
        assert!(rows[1].contains("Go ahead with this plan?") && rows[3].contains("❯ 1. Yes, and accept edits"));

        let mut a = app(false);
        ask(
            &mut a,
            "AskUserQuestion",
            json!({"questions": [{"question": "Which db?", "multiSelect": true,
                "options": [{"label": "sqlite", "description": "file"}, {"label": "pg"}]}]}),
        );
        a.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let (rows, _) = draw(&live_view(&a, 50, 30), 50);
        assert_eq!(rows[1], "│ Which db?                                      │");
        assert_eq!(rows[3], "│ ❯ 1. [x] sqlite  file                          │");
        assert_eq!(rows[5], "│   3. Type an answer                            │");
    }

    #[test]
    fn narrow_terminals_wrap_the_input_and_keep_the_box() {
        let mut a = app(false);
        typed(&mut a, "abcdefghijklmnopqrstuvwxyz");
        let v = live_view(&a, 20, 20);
        let (rows, _) = draw(&v, 20);
        assert_eq!(rows[1], "> abcdefghijklmnopqr");
        assert_eq!(rows[2], "  stuvwxyz");
        assert_eq!(v.cursor, Some((10, 2)));
        assert!(v.lines.iter().all(|l| text::width(&text::plain(l)) <= 20));
        // Moving left goes back across the wrap.
        for _ in 0..9 {
            a.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(live_view(&a, 20, 20).cursor, Some((19, 1)));
        // A dialog stays boxed at 20 columns.
        ask(&mut a, "Write", json!({"file_path": "/a/very/long/path/to/a/file.rs"}));
        let v = live_view(&a, 20, 40);
        assert!(v.lines.iter().all(|l| text::width(&text::plain(l)) == 20 || text::plain(l).starts_with("  ")));
    }

    #[test]
    fn plan_mode_and_hints_show_in_the_status_line() {
        let mut a = app(true);
        a.status.mode = "plan".into();
        let (rows, _) = draw(&live_view(&a, 70, 10), 70);
        assert!(rows.last().unwrap().starts_with("  ⏸ plan mode on (shift+tab to cycle)"));
        a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let (rows, _) = draw(&live_view(&a, 70, 10), 70);
        assert!(rows.last().unwrap().starts_with("  Press Ctrl-C again to exit"));
        let later = Instant::now() + HINT_FOR;
        let (rows, _) = draw(&live_view_at(&a, 70, 10, later), 70);
        assert!(rows.last().unwrap().starts_with("  ⏸ plan mode"));
    }

    #[test]
    fn pickers_search_and_file_menus() {
        use forge_core::commands::picker::{Choice, Pick, Picker};
        let mut a = app(false);
        let c = |l: &str, d: &str, cur: bool| Choice {
            label: l.into(),
            detail: d.into(),
            pick: Pick::Run(String::new()),
            current: cur,
        };
        a.on_event(UiEvent::Picker(Picker {
            title: "Select a model".into(),
            choices: vec![c("Opus", "big", false), c("Haiku", "small", true)],
        }));
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[1], "│ Select a model                       │");
        assert_eq!(rows[2], "│ Type to filter                       │");
        assert_eq!(rows[4], "│   1. Opus  big                       │");
        assert_eq!(rows[5], "│ ❯ 2. Haiku ✔  small                  │");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // Ctrl+R shows the search and the match in the input box.
        let mut a = App::new(Theme { color: false }, vec!["cargo test".into(), "git status".into()]);
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        typed(&mut a, "car");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[0], "  search history: car");
        assert_eq!(rows[2], "> cargo test");
        typed(&mut a, "zz");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[0], "  search history: carzz (no match)");

        // `@` lists matching paths.
        let mut a = app(false);
        a.on_event(UiEvent::Files(vec!["src/".into(), "src/main.rs".into(), "README.md".into()]));
        typed(&mut a, "look at @ma");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[3], "  + src/main.rs");
    }
}
