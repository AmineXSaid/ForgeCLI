//! The live region at the bottom of the terminal, drawn from the app state:
//! the unfinished answer line, the spinner, a dialog, queued messages, the
//! input box, the `/` menu and the status line (docs/TUI.md, "Renderer").

use kodact_core::glyphs;
use std::time::{Duration, Instant};

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::app::App;
use super::front::FrontPage;
use super::palette;
use super::text::{self, width as text_width};

/// The rows to draw, and where the cursor goes (column, row) when it shows.
#[derive(Debug, Clone, Default)]
pub struct LiveView {
    pub lines: Vec<Line<'static>>,
    pub cursor: Option<(u16, u16)>,
}

const SPINNER: [&str; 10] = kodact_core::glyphs::SPINNER;
pub const SPINNER_STEP: Duration = Duration::from_millis(80);
/// How long a hint stays in the status line.
pub const HINT_FOR: Duration = Duration::from_secs(2);
const MENU_ROWS: usize = 8;
const LIVE_ROWS: usize = 3;
/// The composer's placeholder, as KodAct's says it.
pub const PLACEHOLDER: &str = "What shall we build today?";
/// The frame style's placeholder while the model works.
pub const QUEUE_PLACEHOLDER: &str = "Type to queue a message; esc stops the turn";
/// What the spinner says while the model works, in KodAct's voice (its spinner verbs).
pub const VERBS: [&str; 8] =
    ["Acting", "Reading the code", "Thinking", "Building", "Wiring it up", "Checking", "Shaping", "Polishing"];
/// How long one verb stays.
const VERB_FOR: Duration = Duration::from_secs(4);
/// How long the turning dot of a running tool shows one face.
const RUNNING_STEP: Duration = Duration::from_millis(220);

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

fn status_left(app: &App, now: Instant, w: usize) -> (String, Style) {
    if let Some((h, at)) = &app.hint {
        if now.duration_since(*at) < HINT_FOR {
            return (h.clone(), app.theme.dim());
        }
    }
    let t = app.theme;
    // The frame style's panel names the mode, so the line keeps the folder.
    let mode = if frame_panel(app, w) { "" } else { app.status.mode.as_str() };
    match mode {
        "acceptEdits" => (format!("{} accept edits on (shift+tab to cycle)", glyphs::MODE_ACCEPT_EDITS), t.warning()),
        "plan" => (format!("{} plan mode on (shift+tab to cycle)", glyphs::MODE_PLAN), t.success()),
        "bypassPermissions" => (format!("{} bypass permissions on", glyphs::MODE_BYPASS), t.error()),
        "dontAsk" => ("don't ask mode on (shift+tab to cycle)".into(), t.dim()),
        // The folder the session works in; `?` still lists the keys.
        _ if !app.status.cwd.is_empty() => (super::short_path(&app.status.cwd), t.subtle()),
        _ => ("? for shortcuts".into(), t.subtle()),
    }
}

fn status_right(app: &App) -> String {
    if let Some(t) = &app.status_text {
        return t.clone();
    }
    let s = &app.status;
    let mut parts = vec![];
    if !s.model.is_empty() {
        parts.push(s.model.clone());
    }
    match s.context_pct {
        // Some context in use, under 1%: "0%" would read as empty.
        Some(0) if s.context_used => parts.push("<1% context".into()),
        Some(p) => parts.push(format!("{p}% context")),
        None => {}
    }
    parts.extend(spend(app));
    parts.join(" · ")
}

/// What the session has spent: its cost, or, for a model with no known
/// price, the tokens in context (nothing before the first request).
fn spend(app: &App) -> Option<String> {
    let s = &app.status;
    match (s.cost_unknown, s.cost > 0.0) {
        (false, _) => Some(format!("${:.2}", s.cost)),
        // Part of the spend has no known price: show what is known as a lower bound.
        (true, true) => Some(format!("${:.2}+", s.cost)),
        (true, false) => Some(app.context_tokens()).filter(|n| *n > 0).map(tokens),
    }
}

/// `s` in at most `max` columns, its end kept: `…/demo/proj`.
fn keep_end(s: &str, max: usize) -> String {
    if text_width(s) <= max {
        return s.to_string();
    }
    let mut tail: Vec<char> = vec![];
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > max {
            break;
        }
        used += cw;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

/// A permission mode as people say it.
fn mode_name(mode: &str) -> String {
    match mode {
        "acceptEdits" => "Accept edits",
        "plan" => "Plan",
        "auto" => "Auto",
        "dontAsk" => "Don't ask",
        "bypassPermissions" => "Bypass permissions",
        _ => "Default",
    }
    .to_string()
}

/// A token count as people read it: `950 tokens`, `15.5k tokens`, `1.2M tokens`.
fn tokens(n: u64) -> String {
    match n {
        0..=999 => format!("{n} tokens"),
        1_000..=999_999 => format!("{:.1}k tokens", n as f64 / 1000.0).replace(".0k", "k"),
        _ => format!("{:.1}M tokens", n as f64 / 1_000_000.0).replace(".0M", "M"),
    }
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

/// `body` rows between lime corner marks, `w` columns wide: the frame
/// style's way of setting a question apart.
fn cornered(title: Line<'static>, body: Vec<Line<'static>>, w: usize, mark: Style) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4).max(1);
    let edge = |l: &str, r: &str| {
        Line::from(vec![
            Span::styled(l.to_string(), mark),
            Span::raw(" ".repeat(w.saturating_sub(4))),
            Span::styled(r.to_string(), mark),
        ])
    };
    let mut out = vec![edge("┌─", "─┐")];
    for l in std::iter::once(title).chain(body) {
        for row in text::wrap(vec![l], inner as u16) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(row.spans);
            out.push(Line::from(spans));
        }
    }
    out.push(edge("└─", "─┘"));
    out
}

/// A screen's spans, styled.
fn screen_spans(app: &App, spans: &[kodact_core::commands::screens::Span]) -> Vec<Span<'static>> {
    use kodact_core::commands::screens::{context_parts, Tone};
    let t = app.theme;
    spans
        .iter()
        .map(|s| {
            let (text, st) = match s.tone {
                Tone::Plain => (s.text.clone(), Style::default()),
                Tone::Dim => (s.text.clone(), t.dim()),
                Tone::Bold => (s.text.clone(), t.bold()),
                Tone::Accent => (s.text.clone(), t.accent()),
                Tone::Added => (s.text.clone(), t.success()),
                Tone::Removed => (s.text.clone(), t.error()),
                // Without colour, each part has its own letter.
                Tone::Part(p) if !t.color => {
                    let glyph = context_parts().get(p as usize).map(|x| x.1).unwrap_or('?');
                    (s.text.replace(['⛁', '⛶'], &glyph.to_string()), Style::default())
                }
                Tone::Part(p) => (s.text.clone(), t.part(p)),
            };
            Span::styled(text, st)
        })
        .collect()
}

/// Rows a screen's box takes besides its rows: borders, title, blank, footer.
const SCREEN_CHROME: usize = 5;

/// A screen, at most `room` rows tall. Rows are cut to the width, never wrapped, so
/// scrolling counts rows exactly.
fn viewer(app: &App, w: usize, room: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let Some((screen, cursor, top)) = app.viewer() else { return vec![] };
    let page = room.saturating_sub(SCREEN_CHROME).max(1);
    app.viewer_page.set(page);
    let inner = w.saturating_sub(6).max(1);
    // The highlighted row stays in view even if the terminal shrank.
    let top = top.min(cursor).max((cursor + 1).saturating_sub(page));
    // A screen with nothing to choose scrolls; it highlights no row.
    let pick = screen.rows.iter().any(|r| r.action.is_some());
    let cursor = if pick { cursor } else { usize::MAX };
    let mut body = vec![];
    for (i, row) in screen.rows.iter().enumerate().skip(top).take(page) {
        let mark = if i == cursor { "❯ " } else { "  " };
        let mut spans = vec![Span::styled(mark, t.accent())];
        let mut used = 0;
        for sp in screen_spans(app, &row.spans) {
            let room = inner.saturating_sub(used);
            if room == 0 {
                break;
            }
            let text = fit(&sp.content, room);
            used += text_width(&text);
            // The highlighted row keeps its colours (diff sides, context parts) and turns bold.
            let st = if i == cursor { sp.style.add_modifier(ratatui::style::Modifier::BOLD) } else { sp.style };
            spans.push(Span::styled(text, st));
        }
        body.push(Line::from(spans));
    }
    let n = screen.rows.len();
    let shown = if n > page { format!("{}-{} of {n} · ", top + 1, (top + page).min(n)) } else { String::new() };
    let hint = match screen.rows.get(cursor).and_then(|r| r.action.as_ref()) {
        Some(kodact_core::commands::screens::RowAction::Jump(_)) => "Enter open · ",
        Some(_) => "Enter choose · ",
        None => "",
    };
    body.push(Line::default());
    body.push(Line::from(Span::styled(
        fit(&format!("{shown}↑↓ PgUp PgDn · {hint}Esc back"), w.saturating_sub(4)),
        t.dim(),
    )));
    boxed(Line::from(Span::styled(fit(&screen.title, w.saturating_sub(4)), t.bold())), body, w, t.accent())
}

/// A form: one row per field, the command it will run, the keys.
fn form(app: &App, w: usize) -> Vec<Line<'static>> {
    use kodact_core::commands::screens::FieldKind;
    let t = app.theme;
    let Some((form, at)) = app.form() else { return vec![] };
    let pad = form.fields.iter().map(|f| text_width(&f.label)).max().unwrap_or(0);
    let mut body = vec![];
    for (i, f) in form.fields.iter().enumerate() {
        let here = i == at;
        let mut spans = vec![
            Span::styled(if here { "❯ " } else { "  " }, t.accent()),
            Span::styled(format!("{:<pad$}  ", f.label), if here { t.bold() } else { Style::default() }),
        ];
        match &f.kind {
            FieldKind::Text(v) => {
                spans.push(Span::raw(v.clone()));
                if here {
                    spans.push(Span::styled("▏", t.accent()));
                }
            }
            FieldKind::Choice { options, at: c } => {
                let v = options.get(*c).cloned().unwrap_or_default();
                spans.push(Span::styled(if here { format!("‹ {v} ›") } else { v }, Style::default()));
            }
            FieldKind::Multi { options, picked, at: c } => {
                for (j, (o, p)) in options.iter().zip(picked).enumerate() {
                    let st = if here && j == *c { t.selected() } else { Style::default() };
                    spans.push(Span::styled(format!("[{}] {o}", if *p { "x" } else { " " }), st));
                    spans.push(Span::raw(" "));
                }
            }
        }
        body.push(Line::from(spans));
    }
    body.push(Line::default());
    body.push(Line::from(Span::styled(format!("Runs: {}", form.command()), t.dim())));
    body.push(Line::from(Span::styled("Tab next field · ←→ choose · Space toggle · Enter run · Esc cancel", t.dim())));
    boxed(Line::from(Span::styled(form.title.clone(), t.bold())), body, w, t.accent())
}

fn dialog(app: &App, w: usize, room: usize) -> Vec<Line<'static>> {
    if app.viewer().is_some() {
        return viewer(app, w, room);
    }
    if app.form().is_some() {
        return form(app, w);
    }
    let t = app.theme;
    let (title, body) = app.dialog_text();
    // A proposed edit's lines: removed in red, added in green.
    let mut rows: Vec<Line<'static>> = body
        .into_iter()
        .map(|l| {
            if l.starts_with("  - ") {
                Line::from(Span::styled(l, t.removed()))
            } else if l.starts_with("  + ") {
                Line::from(Span::styled(l, t.added()))
            } else {
                Line::from(l)
            }
        })
        .collect();
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
    let permission = app.dialog_options().first().is_some_and(|(_, k)| k == "y");
    let hint =
        if permission { "Enter or y·a·n to answer · Esc to deny" } else { "Enter to select · Esc to cancel" };
    rows.push(Line::from(Span::styled(hint, t.dim())));
    let title = if permission {
        Line::from(vec![Span::styled("◆ ", t.accent()), Span::styled(title, t.bold())])
    } else {
        Line::from(Span::styled(title, t.bold()))
    };
    use super::look::Look;
    match app.look {
        // Lime corner marks around the question, no full frame.
        Look::Frame => cornered(title, rows, w, t.accent()),
        // A centred sheet, narrower than the terminal.
        Look::Status if w >= 60 => {
            let bw = w.min(76);
            let pad = " ".repeat((w - bw) / 2);
            boxed(title, rows, bw, t.accent())
                .into_iter()
                .map(|l| {
                    let mut spans = vec![Span::raw(pad.clone())];
                    spans.extend(l.spans);
                    Line::from(spans)
                })
                .collect()
        }
        // No box: a rule, then the question.
        Look::Bare => {
            let mut out = vec![Line::from(Span::styled("─".repeat(w), t.hairline()))];
            out.extend(text::wrap(std::iter::once(title).chain(rows).collect(), w as u16));
            out
        }
        _ => boxed(title, rows, w, t.accent()),
    }
}

/// The input box's rows and the cursor's (column, row) inside them.
fn input(app: &App, w: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let t = app.theme;
    let text_w = w.saturating_sub(2).max(1);
    let bare = app.look == super::look::Look::Bare;
    let prompt = || {
        if bare {
            Span::styled("› ", t.subtle())
        } else {
            Span::styled(format!("{} ", glyphs::PROMPT), t.accent())
        }
    };
    if app.editor.is_empty() && !app.busy {
        let row = Line::from(vec![prompt(), Span::styled(fit(PLACEHOLDER, text_w), t.dim())]);
        return (vec![row], (2, 0));
    }
    // While the model works, the frame style's panel says what typing does.
    if app.editor.is_empty() && app.look == super::look::Look::Frame {
        let row = Line::from(vec![prompt(), Span::styled(fit(QUEUE_PLACEHOLDER, text_w), t.dim())]);
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
            let prefix = if rows.is_empty() { prompt() } else { Span::raw("  ") };
            rows.push(Line::from(vec![prefix, Span::raw(chars[*a..*b].iter().collect::<String>())]));
        }
    }
    (rows, cursor)
}

/// The permission mode on the frame style's panel, by its name
/// ([`mode_name`]), in its colour: default in lime, plan in teal, auto in
/// violet, accept edits in amber, bypass permissions in red, don't ask quiet.
fn mode_mark(app: &App) -> (String, Style) {
    (mode_name(&app.status.mode), app.theme.mode(&app.status.mode))
}

/// The frame style's input, `bw` columns wide from column `left`: a raised
/// panel, its left edge in the mode's colour, the text, then the mode, the
/// model, where it runs and the context in use, with a row of padding above
/// and below: one solid block, whatever the terminal's font. The cursor's
/// (column, row) inside it.
fn panel(app: &App, left: usize, bw: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let t = app.theme;
    let margin = || Span::raw(" ".repeat(left));
    let raised = t.raised();
    let (mode, mode_st) = mode_mark(app);
    let edge = if t.color { Span::styled("▎", mode_st.patch(raised)) } else { Span::styled("│", t.bold()) };
    let inner = bw.saturating_sub(4).max(1);
    let mut out = vec![];
    let row = |spans: Vec<Span<'static>>, out: &mut Vec<Line<'static>>| {
        let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
        let mut line = vec![margin(), edge.clone(), Span::styled(" ", raised)];
        line.extend(spans.into_iter().map(|s| Span::styled(s.content, raised.patch(s.style))));
        line.push(Span::styled(" ".repeat((inner + 2).saturating_sub(used)), raised));
        out.push(Line::from(line));
    };
    let (rows, cur) = input(app, inner);
    if t.color {
        row(vec![], &mut out);
    }
    let first = out.len() as u16;
    for r in rows {
        // The edge is the prompt: the marker gives way to two spaces. What is
        // typed takes the person's colour; the placeholder keeps its own.
        let mut spans: Vec<Span<'static>> =
            r.spans.into_iter().map(|s| Span::styled(s.content, t.user_text().patch(s.style))).collect();
        if !spans.is_empty() {
            spans[0] = Span::raw("  ");
        }
        row(spans, &mut out);
    }
    // A row of air, then the mode, the model and where it runs; the context in use on the right.
    row(vec![], &mut out);
    let s = &app.status;
    let sep = || Span::raw("  ");
    let mut meta = vec![Span::raw("  "), Span::styled(mode, mode_st.add_modifier(ratatui::style::Modifier::BOLD))];
    // The mode is what this row is for: never dropped. The endpoint goes first,
    // then the context, then the model is shortened to what is left.
    let model_room = (inner + 2).saturating_sub(meta.iter().map(|s| text_width(&s.content)).sum::<usize>() + 2);
    if !s.model.is_empty() && model_room >= 4 {
        meta.extend([sep(), Span::raw(fit(&s.model, model_room))]);
    }
    let endpoint = vec![sep(), Span::styled(app.endpoint.clone(), t.subtle())];
    let mut right = vec![];
    if let Some(pct) = s.context_pct.filter(|p| *p > 0 || s.context_used) {
        right.extend(meter(&t, pct));
        let label = if pct == 0 && s.context_used { " <1%".to_string() } else { format!(" {pct}%") };
        right.push(Span::styled(label, t.dim()));
    }
    if let Some(cost) = spend(app).filter(|_| !right.is_empty()) {
        right.push(Span::styled(format!(" · {cost}"), t.subtle()));
    }
    let w_of = |v: &[Span<'static>]| v.iter().map(|s| text_width(&s.content)).sum::<usize>();
    let room = inner + 2;
    if !app.endpoint.is_empty() && w_of(&meta) + w_of(&endpoint) + w_of(&right) + 4 <= room {
        meta.extend(endpoint);
    }
    if w_of(&meta) + w_of(&right) + 2 > room {
        right.clear();
    }
    let gap = room.saturating_sub(w_of(&meta) + w_of(&right));
    meta.push(Span::raw(" ".repeat(gap.saturating_sub(2))));
    meta.extend(right);
    row(meta, &mut out);
    if t.color {
        row(vec![], &mut out);
    }
    (out, (left as u16 + 2 + cur.0, first + cur.1))
}

/// The keys that help now, for the frame style's status line: while the model
/// works, how to stop it; in a menu, how to choose; with nothing typed, the
/// commands, files and modes; while typing, how to send.
fn hints(app: &App, room: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let keys: &[(&str, &str)] = if app.dialog.is_some() {
        // The dialog says its own keys.
        &[]
    } else if app.busy {
        &[("esc", "stop"), ("enter", "queue a message")]
    } else if !app.menu().is_empty() || !app.file_menu().is_empty() {
        &[("↑↓", "choose"), ("tab", "complete"), ("esc", "close")]
    } else if app.editor.is_empty() {
        &[("/", "commands"), ("@", "files"), ("shift+tab", "mode"), ("?", "keys")]
    } else {
        &[("enter", "send"), ("shift+enter", "new line")]
    };
    let mut out: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for (i, (k, what)) in keys.iter().enumerate() {
        let gap = if i == 0 { "" } else { "   " };
        let w = text_width(gap) + text_width(k) + 1 + text_width(what);
        if used + w > room {
            break;
        }
        used += w;
        out.push(Span::raw(gap));
        out.push(Span::styled(k.to_string(), if t.color { Style::default() } else { t.bold() }));
        out.push(Span::styled(format!(" {what}"), t.subtle()));
    }
    out
}

/// Menu rows under the frame style's panel, `bw` wide from column `left`: on
/// the panel's raised surface, past a hairline edge, so they read as part of it.
fn attached(app: &App, rows: Vec<Line<'static>>, left: usize, bw: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    if rows.is_empty() || !t.color {
        return rows.into_iter().map(|l| at_col(left, l)).collect();
    }
    let raised = t.raised();
    let mut out: Vec<Line<'static>> = rows
        .into_iter()
        .map(|l| {
            let used: usize = l.spans.iter().map(|s| text_width(&s.content)).sum();
            let mut spans = vec![Span::raw(" ".repeat(left)), Span::styled("▎ ", t.hairline().patch(raised))];
            spans.extend(l.spans.into_iter().map(|s| Span::styled(s.content, raised.patch(s.style))));
            spans.push(Span::styled(" ".repeat(bw.saturating_sub(2 + used)), raised));
            Line::from(spans)
        })
        .collect();
    // A row of padding under the last choice, like the panel's own.
    out.push(Line::from(vec![
        Span::raw(" ".repeat(left)),
        Span::styled("▎ ", t.hairline().patch(raised)),
        Span::styled(" ".repeat(bw.saturating_sub(2)), raised),
    ]));
    out
}

fn at_col(x: usize, l: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(x))];
    spans.extend(l.spans);
    Line::from(spans)
}

/// Whether the input is the frame style's panel: the frame style, with room for it.
fn frame_panel(app: &App, w: usize) -> bool {
    app.look == super::look::Look::Frame && w >= 40
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
            Line::from(Span::styled(fit(&format!("  {f}"), w), st))
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

/// KodAct's working shimmer: a soft highlight sweeps across `text`, once
/// every two seconds or so.
fn shimmer(t: &text::Theme, text: &str, since: Duration) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let span = chars.len() as i64 + 8;
    let head = ((since.as_millis() / 70) as i64 % span) - 4;
    let mut out: Vec<Span<'static>> = vec![];
    for (i, c) in chars.iter().enumerate() {
        let d = (i as i64 - head).abs();
        let level = match d {
            0 => 3,
            1 => 2,
            2 => 1,
            _ => 0,
        };
        let st = t.shimmer(level);
        match out.last_mut() {
            Some(s) if s.style == st => s.content.to_mut().push(*c),
            _ => out.push(Span::styled(c.to_string(), st)),
        }
    }
    out
}

/// The context meter: `━━━─────` (box-drawing bars, which every monospace font
/// has), brand while there is room, amber from 70%, red from 90%.
fn meter(t: &text::Theme, pct: u8) -> Vec<Span<'static>> {
    const CELLS: usize = 8;
    let filled = ((pct as usize * CELLS + 50) / 100).min(CELLS);
    let tone = if pct >= 90 {
        t.error()
    } else if pct >= 70 {
        t.warning()
    } else {
        t.accent()
    };
    vec![Span::styled("━".repeat(filled), tone), Span::styled("─".repeat(CELLS - filled), t.subtle())]
}

fn status(app: &App, w: usize, now: Instant) -> Line<'static> {
    let (left, left_style) = status_left(app, now, w);
    let lead = 2;
    // The folder keeps its end and at most half the line, so the model and context stay in view.
    let folder = !app.status.cwd.is_empty() && left == super::short_path(&app.status.cwd);
    let mut left = if folder { keep_end(&left, (w / 2).max(8)) } else { fit(&left, w.saturating_sub(lead)) };
    if folder && app.look == super::look::Look::Frame && !app.branch.is_empty() {
        left = format!("{left}  ⎇ {}", fit(&app.branch, 24));
    }
    let room = w.saturating_sub(text_width(&left) + lead + 2);
    // The front screen shows the model under its input; the status line keeps the version.
    let right = if frame_panel(app, w) && app.status_text.is_none() {
        // The keys that help now, on the front page too: it is where they are needed most.
        hints(app, room)
    } else if app.front && app.status_text.is_none() {
        vec![Span::styled(format!("v{}", kodact_core::VERSION), app.theme.subtle())]
    } else if frame_panel(app, w) && app.status_text.is_none() {
        // The panel shows the model and the context; the line keeps the keys that help now.
        hints(app, room)
    } else {
        status_right_spans(app, room)
    };
    let right_w: usize = right.iter().map(|s| text_width(&s.content)).sum();
    let gap = w.saturating_sub(lead + text_width(&left) + right_w);
    let mut spans = vec![Span::raw("  ")];
    spans.extend([Span::styled(left, left_style), Span::raw(" ".repeat(gap))]);
    spans.extend(right);
    Line::from(spans)
}

/// The right side of the status line in `room` columns: with the context
/// meter when it fits, else the plain text of [`status_right`].
fn status_right_spans(app: &App, room: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let s = &app.status;
    let plain = status_right(app);
    if app.status_text.is_some() || s.context_pct.is_none() || text_width(&plain) + 2 > room {
        return if room >= 8 { vec![Span::styled(fit(&plain, room), t.dim())] } else { vec![] };
    }
    let sep = || Span::styled(" · ", t.hairline());
    let mut out = vec![];
    if !s.model.is_empty() {
        out.push(Span::styled(s.model.clone(), t.dim()));
        out.push(sep());
    }
    let pct = s.context_pct.unwrap_or(0);
    out.extend(meter(&t, pct));
    let label = if pct == 0 && s.context_used { " <1%".to_string() } else { format!(" {pct}%") };
    out.push(Span::styled(label, t.dim()));
    if let Some(cost) = spend(app) {
        out.push(sep());
        out.push(Span::styled(cost, t.dim()));
    }
    let w: usize = out.iter().map(|s| text_width(&s.content)).sum();
    if w > room {
        return vec![Span::styled(fit(&plain, room), t.dim())];
    }
    out
}

/// The whole screen for the full-screen UI, `width` by `height`. Before the
/// first message, the style's front page (`front.rs`): its header at the top,
/// then its rows with the input among them, centred down the screen (or from
/// the top, or with the input docked at the bottom), and the status line on
/// the last row. After it, the conversation: `rows` (the transcript, already
/// wrapped to `width`) fills the screen down to the live region, `scroll` rows
/// up from its end; the status style adds a bar of readouts on top. Returns
/// the view and the furthest it can scroll.
pub fn full_view(
    app: &App,
    width: u16,
    height: u16,
    rows: &[Line<'static>],
    page: &FrontPage,
    scroll: usize,
) -> (LiveView, usize) {
    full_view_at(app, width, height, rows, page, scroll, Instant::now())
}

pub fn full_view_at(
    app: &App,
    width: u16,
    height: u16,
    rows: &[Line<'static>],
    page: &FrontPage,
    scroll: usize,
    now: Instant,
) -> (LiveView, usize) {
    let h = (height as usize).max(1);
    if app.front && app.dialog.is_none() && !app.busy {
        let fb = FrontBox { left: page.box_left, w: page.box_w, under: page.under };
        let live = live_view_inner(app, width, height, now, Some(fb));
        let mut input = live.lines;
        let status = input.pop().unwrap_or_default();
        let room = h.saturating_sub(1);
        let notes: Vec<Line<'static>> = rows.iter().rev().take(6).rev().cloned().collect();
        let mut out: Vec<Line<'static>> = page.header.clone();
        let box_at;
        if page.dock {
            // The page from the top, the input at the bottom.
            out.extend(page.above.iter().cloned());
            if !notes.is_empty() {
                out.push(Line::default());
                out.extend(notes);
            }
            out.truncate(room.saturating_sub(input.len()));
            out.resize(room.saturating_sub(input.len()), Line::default());
            box_at = out.len();
            out.extend(input);
        } else {
            let mut mid: Vec<Line<'static>> = vec![];
            // The style's art goes first when the screen is too short for it all.
            let fits = page.header.len() + page.above.len() + input.len() + page.below.len() <= room;
            if fits {
                mid.extend(page.above.iter().cloned());
            }
            let at = mid.len();
            mid.extend(input);
            if mid.len() + page.below.len() + page.header.len() <= room {
                mid.extend(page.below.iter().cloned());
            }
            if !notes.is_empty() && page.header.len() + mid.len() + 1 + notes.len() <= room {
                mid.push(Line::default());
                mid.extend(notes);
            }
            let free = room.saturating_sub(page.header.len() + mid.len());
            // Centred down the screen, the empty rows split evenly above and below; or from the top.
            let top = if page.top { free.min(1) } else { free / 2 };
            out.extend(vec![Line::default(); top]);
            box_at = out.len() + at;
            out.extend(mid);
        }
        out.truncate(room);
        out.resize(room, Line::default());
        out.push(status);
        let cursor = live.cursor.map(|(x, y)| (x, y + box_at as u16)).filter(|(_, y)| (*y as usize) < room);
        return (LiveView { lines: out, cursor }, 0);
    }
    let live = live_view_at(app, width, height, now);
    // The status style: a bar of the session's readouts across the top.
    let bar: Vec<Line<'static>> =
        if app.look == super::look::Look::Status { vec![readouts(app, width as usize)] } else { vec![] };
    // The conversation: the transcript down to the live region.
    let avail = h.saturating_sub(live.lines.len() + bar.len());
    let max_scroll = rows.len().saturating_sub(avail);
    let scroll = scroll.min(max_scroll);
    let end = rows.len() - scroll;
    let mut out: Vec<Line<'static>> = rows[end.saturating_sub(avail)..end].to_vec();
    if scroll > 0 && !out.is_empty() {
        let t = app.theme;
        let note = format!("↓ {scroll} more {} · PgDn", if scroll == 1 { "line" } else { "lines" });
        let pad = (width as usize).saturating_sub(text_width(&note)) / 2;
        let last = out.len() - 1;
        out[last] = Line::from(vec![Span::raw(" ".repeat(pad)), Span::styled(note, t.subtle())]);
    }
    // A short transcript sits just above the live region, as one flow with the input.
    if out.len() < avail {
        let mut padded = vec![Line::default(); avail - out.len()];
        padded.extend(out);
        out = padded;
    }
    let mut lines = bar;
    let cursor = live.cursor.map(|(x, y)| (x, y + (lines.len() + avail) as u16));
    lines.extend(out);
    lines.extend(live.lines);
    (LiveView { lines, cursor }, max_scroll)
}

/// The status style's bar: the model, the context in use, the folder and the branch.
fn readouts(app: &App, w: usize) -> Line<'static> {
    let t = app.theme;
    let raised =
        if t.color { Style::default().bg(palette::PROMPT_BG.color(t.depth, t.light)) } else { Style::default() };
    let mut parts: Vec<(String, String)> =
        vec![("model".into(), fit(&format!("{} · {}", app.status.model, app.endpoint), (w / 3).max(12)))];
    if let Some(p) = app.status.context_pct {
        parts.push(("context".into(), format!("{p}%")));
    }
    parts.push(("folder".into(), keep_end(&super::short_path(&app.status.cwd), (w / 4).max(12))));
    if !app.branch.is_empty() {
        parts.push(("branch".into(), app.branch.clone()));
    }
    let mut spans = vec![Span::styled(" ", raised)];
    for (i, (k, v)) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  │  ", t.hairline().patch(raised)));
        }
        spans.push(Span::styled(format!("{k} "), t.subtle().patch(raised)));
        spans.push(Span::styled(v.clone(), Style::default().patch(raised)));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used < w {
        spans.push(Span::styled(" ".repeat(w - used), raised));
    }
    // One row, never wrapped: cut at the width.
    let mut out = vec![];
    let mut room = w;
    for sp in spans {
        if room == 0 {
            break;
        }
        let text = if text_width(&sp.content) > room { fit(&sp.content, room) } else { sp.content.to_string() };
        room -= text_width(&text).min(room);
        out.push(Span::styled(text, sp.style));
    }
    Line::from(out)
}

/// Where the front page puts the input: its first column, its width, and
/// whether the mode, model and endpoint go on a line under it.
#[derive(Debug, Clone, Copy)]
pub struct FrontBox {
    pub left: usize,
    pub w: usize,
    pub under: bool,
}

/// The live region for a terminal `width` columns wide, at most `max_height` rows.
pub fn live_view(app: &App, width: u16, max_height: u16) -> LiveView {
    live_view_at(app, width, max_height, Instant::now())
}

pub fn live_view_at(app: &App, width: u16, max_height: u16, now: Instant) -> LiveView {
    live_view_inner(app, width, max_height, now, None)
}

fn live_view_inner(app: &App, width: u16, max_height: u16, now: Instant, front_box: Option<FrontBox>) -> LiveView {
    let w = (width as usize).max(4);
    let t = app.theme;
    // 1. The unfinished answer line: its last rows.
    let mut live: Vec<Line<'static>> = app.live_line().map(|l| text::wrap(vec![l], w as u16)).unwrap_or_default();
    if live.len() > LIVE_ROWS {
        live.drain(..live.len() - LIVE_ROWS);
    }
    // 2. Tool calls still running, each with its dot turning; then the spinner.
    let mut spinner = vec![];
    for r in &app.running {
        let face = glyphs::RUNNING[(now.duration_since(r.since).as_millis() / RUNNING_STEP.as_millis()) as usize % 4];
        let mut spans = vec![Span::styled(format!("{face} "), t.accent())];
        spans.extend(r.header.iter().cloned());
        spinner.push(Line::default());
        spinner.extend(text::wrap(vec![Line::from(spans)], w as u16).into_iter().take(2));
    }
    if app.busy {
        let since = app.busy_since.map(|s| now.duration_since(s)).unwrap_or_default();
        let frame = SPINNER[(since.as_millis() / SPINNER_STEP.as_millis()) as usize % SPINNER.len()];
        let mut line = vec![Span::styled(format!("{frame} "), t.accent())];
        if app.waiting_for_answer() {
            // Needs you: the brand colour, and how long you have been asked, not the turn's time.
            let waited = app.asked_at.map(|a| now.saturating_duration_since(a)).unwrap_or_default().as_secs();
            line[0] = Span::styled("◆ ", t.accent());
            line.push(Span::styled("Waiting for your answer", Style::default()));
            line.push(Span::styled(
                fit(&format!(" ({waited}s · ↑↓ to choose, Enter to answer)"), w.saturating_sub(25)),
                t.subtle(),
            ));
        } else {
            let verb = match app.activity.as_str() {
                "" | "Working" => VERBS[(since.as_secs() / VERB_FOR.as_secs()) as usize % VERBS.len()].to_string(),
                a => a.to_string(),
            };
            line.extend(shimmer(&t, &format!("{verb}…"), since));
            let rest = format!(" ({}s · esc to interrupt)", since.as_secs());
            let room = w.saturating_sub(2 + text_width(&verb) + 1);
            line.push(Span::styled(fit(&rest, room), t.subtle()));
        }
        spinner.push(Line::default());
        spinner.push(Line::from(line));
    }
    // 3. A dialog.
    let room = (max_height as usize).max(1).saturating_sub(spinner.len() + 1);
    let dialog = if app.dialog.is_some() { dialog(app, w, room) } else { vec![] };
    // 4. Queued messages.
    let mut queued: Vec<Line<'static>> = app
        .queued
        .iter()
        .map(|q| Line::from(Span::styled(fit(&format!("  ⏎ {}", q.replace('\n', " ")), w), t.dim())))
        .collect();
    // 5. The input box (hidden while a dialog is open).
    // The input sits in a rounded box; its rows are padded to the box's inside. On the
    // front screen the box is centred under the wordmark, at most 80 columns, with the
    // mode, the model and where it runs under it.
    let fb = front_box.or_else(|| {
        (app.front && w >= 60).then(|| {
            let bw = (w - 4).min(80);
            FrontBox { left: (w - bw) / 2, w: bw, under: true }
        })
    });
    let fb = fb.filter(|_| app.front && app.dialog.is_none());
    let front = fb.is_some();
    let (left, bw) = fb.map(|b| (b.left.min(w.saturating_sub(4)), b.w.min(w))).unwrap_or((0, w));
    let margin = " ".repeat(left);
    use super::look::Look;
    // Bare and Ledger draw the input under a rule, without a box; Glow's box glows lime.
    let open = matches!(app.look, Look::Bare | Look::Ledger);
    let border = match app.look {
        Look::Glow => t.accent(),
        _ if app.editor.is_empty() => t.hairline(),
        _ => t.subtle(),
    };
    let side = if open { 0 } else { 2 };
    let inner = bw.saturating_sub(2 * side).max(1);
    let edge = |l: char, r: char| {
        if open {
            Line::from(vec![Span::raw(margin.clone()), Span::styled("─".repeat(bw), border)])
        } else {
            Line::from(vec![
                Span::raw(margin.clone()),
                Span::styled(format!("{l}{}{r}", "─".repeat(bw.saturating_sub(2))), border),
            ])
        }
    };
    let (top, bottom) = (edge('╭', '╮'), edge('╰', '╯'));
    // The frame style: a raised panel instead of the box.
    let panel = (frame_panel(app, bw) && app.dialog.is_none()).then(|| panel(app, left, bw));
    let (input_rows, cursor) = if app.dialog.is_none() { input(app, inner) } else { (vec![], (0, 0)) };
    let input_rows: Vec<Line<'static>> = input_rows
        .into_iter()
        .map(|row| {
            let used: usize = row.spans.iter().map(|s| text_width(&s.content)).sum();
            let mut spans = vec![Span::raw(margin.clone())];
            if !open {
                spans.push(Span::styled("│ ", border));
            }
            spans.extend(row.spans);
            if !open {
                spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
                spans.push(Span::styled(" │", border));
            }
            Line::from(spans)
        })
        .collect();
    let cursor = (cursor.0 + side as u16 + margin.len() as u16, cursor.1);
    let under: Vec<Line<'static>> = if panel.is_none() && front && fb.is_some_and(|b| b.under) && !input_rows.is_empty()
    {
        let mut parts = vec![Span::styled(mode_name(&app.status.mode), t.accent())];
        for p in [app.status.model.as_str(), app.endpoint.as_str()].into_iter().filter(|p| !p.is_empty()) {
            parts.push(Span::styled("  ·  ", t.hairline()));
            parts.push(Span::styled(p.to_string(), t.subtle()));
        }
        let used: usize = parts.iter().map(|s| text_width(&s.content)).sum();
        if used + 4 <= w {
            let mut spans = vec![Span::raw(" ".repeat((w - used) / 2))];
            spans.extend(parts);
            vec![Line::from(spans)]
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    // Ctrl+R: what is being searched for, above the input.
    let search: Vec<Line<'static>> = app
        .search
        .as_ref()
        .map(|s| {
            let miss = if s.hit.is_none() && !s.query.is_empty() { " (no match)" } else { "" };
            let pad = if frame_panel(app, bw) { left } else { 0 };
            Line::from(vec![
                Span::raw(" ".repeat(pad)),
                Span::styled(fit(&format!("  search history: {}{miss}", s.query), w.saturating_sub(pad)), t.accent()),
            ])
        })
        .into_iter()
        .collect();
    // 6. The `/` or `@` menu, 7. the status line. Under the frame style's panel,
    // the menu continues it: the same column, the same raised surface.
    let menu = match &panel {
        Some(_) => attached(app, menu(app, bw.saturating_sub(4)), left, bw),
        None => menu(app, w),
    };
    let status = status(app, w, now);

    let fixed = spinner.len()
        + search.len()
        + dialog.len()
        + match &panel {
            Some((rows, _)) => rows.len(),
            None if input_rows.is_empty() => 0,
            None => input_rows.len() + if open { 1 } else { 2 },
        }
        + under.len()
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
    if let Some((rows, c)) = panel {
        cur = Some((c.0, lines.len() as u16 + c.1));
        lines.extend(rows);
    } else if !input_rows.is_empty() {
        lines.push(top);
        cur = Some((cursor.0, (lines.len() as u16) + cursor.1));
        lines.extend(input_rows);
        if !open {
            lines.push(bottom);
        }
        lines.extend(under);
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
    use kodact_engine::PermissionPrompt;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;
    use serde_json::json;

    #[test]
    fn unknown_cost_is_not_shown_as_zero() {
        let mut a = app(false);
        a.on_event(UiEvent::Status(StatusView {
            model: "local".into(),
            mode: "default".into(),
            cwd: "/p".into(),
            cost: 0.0,
            cost_unknown: true,
            context_pct: Some(3),
            context_used: true,
            window: 200_000,
        }));
        assert_eq!(status_right(&a), "local · 3% context", "no price and nothing used yet: nothing to show");
        a.status.context_pct = Some(0);
        assert_eq!(status_right(&a), "local · <1% context");
        a.status.context_used = false;
        assert_eq!(status_right(&a), "local · 0% context");
        // A model with no price shows the tokens it used.
        assert_eq!(tokens(950), "950 tokens");
        assert_eq!(tokens(15_500), "15.5k tokens");
        assert_eq!(tokens(2_000), "2k tokens");
        assert_eq!(tokens(1_200_000), "1.2M tokens");
    }

    fn app(color: bool) -> App {
        let mut a = App::new(Theme { color, light: false, accent: None, depth: Default::default() }, vec![]);
        // The rounded boxes these tests were written for.
        a.look = crate::tui::look::Look::Card;
        a.on_event(UiEvent::Status(StatusView {
            model: "opus".into(),
            mode: "default".into(),
            cwd: "/p".into(),
            cost: 0.25,
            cost_unknown: false,
            context_pct: Some(12),
            context_used: true,
            window: 200_000,
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
        assert_eq!(rows[0], format!("╭{}╮", "─".repeat(58)));
        assert_eq!(rows[1], format!("│ ❯ {PLACEHOLDER:<54} │"));
        assert_eq!(rows[2], format!("╰{}╯", "─".repeat(58)));
        assert_eq!(rows[3], "  /p                             opus · ━─────── 12% · $0.25");
        assert_eq!(v.cursor, Some((4, 1)));
        // The placeholder is quiet but readable (muted, over 4.5:1); the prompt marker is KodAct's lime.
        assert_eq!(buf[(6, 1)].fg, crate::tui::palette::MUTED.color(Default::default(), false));
        assert_eq!(buf[(2, 1)].fg, crate::tui::palette::BRAND.color(Default::default(), false));
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
        assert_eq!(rows[0], "● partial answer");
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "⠦ Acting… (1s · esc to interrupt)", "1.3 s is step 16: frame 7 of 10");
        assert_eq!(rows[3], "  ⏎ next");
        assert!(rows[5].starts_with("│ ❯ ab ") && rows[5].ends_with('│'), "{}", rows[5]);
        assert_eq!(v.cursor, Some((6, 5)));
        // Too short a screen drops the answer line first, then queued messages, never the input.
        let v = live_view_at(&a, 50, 6, start);
        let (rows, _) = draw(&v, 50);
        assert!(rows.iter().all(|r| !r.contains("partial")), "{rows:?}");
        assert!(rows.iter().any(|r| r.starts_with("│ ❯ ab")), "{rows:?}");
        assert_eq!(v.lines.len(), 6);
    }

    #[test]
    fn the_menu_lists_matching_commands() {
        let mut a = app(true);
        typed(&mut a, "/c");
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let v = live_view(&a, 60, 20);
        let (rows, buf) = draw(&v, 60);
        assert!(rows[1].starts_with("│ ❯ /c "), "{}", rows[1]);
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
                "│ ◆ Approve Bash?                      │",
                "│   rm -rf build                       │",
                "│                                      │",
                "│ ❯ 1. Allow once  y                   │",
                "│   2. Deny, and tell KodAct what to   │",
                "│      do  n                           │",
                "│                                      │",
                "│ Enter or y·a·n to answer · Esc to    │",
                "│ deny                                 │",
                "╰──────────────────────────────────────╯",
                "  /p         opus · ━─────── 12% · $0.25",
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
        assert_eq!(rows[1], "│ ❯ abcdefghijklmn │");
        assert_eq!(rows[2], "│   opqrstuvwxyz   │");
        assert_eq!(v.cursor, Some((16, 2)));
        assert!(v.lines.iter().all(|l| text::width(&text::plain(l)) <= 20));
        // Moving left goes back across the wrap.
        for _ in 0..9 {
            a.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(live_view(&a, 20, 20).cursor, Some((7, 2)));
        // A dialog stays boxed at 20 columns.
        ask(&mut a, "Write", json!({"file_path": "/a/very/long/path/to/a/file.rs"}));
        let v = live_view(&a, 20, 40);
        assert!(v.lines.iter().all(|l| text::width(&text::plain(l)) == 20 || text::plain(l).starts_with("  ")));
    }

    #[test]
    fn the_frame_style_draws_a_panel_in_the_mode_colour_with_helpful_keys() {
        let mut a = app(true);
        a.look = crate::tui::look::Look::Frame;
        a.endpoint = "127.0.0.1:11434".into();
        let view = live_view(&a, 80, 12);
        let (rows, buf) = draw(&view, 80);
        let n = rows.len();
        // A row of padding, the text, a row of air, the mode line, a row of padding; then the keys.
        assert_eq!(rows[n - 6].trim_end(), "▎", "{rows:#?}");
        assert!(rows[n - 5].starts_with("▎   What shall we build today?"), "{rows:#?}");
        assert_eq!(rows[n - 4].trim_end(), "▎", "a row between the text and the mode");
        assert!(rows[n - 3].starts_with("▎   Default  opus  127.0.0.1:11434"), "{rows:#?}");
        assert!(rows[n - 3].trim_end().ends_with("12% · $0.25"), "{rows:#?}");
        assert_eq!(rows[n - 2].trim_end(), "▎");
        // Every cell of the panel on the raised surface: no stripes.
        let raised = Some(crate::tui::palette::PROMPT_BG.color(a.theme.depth, false));
        for y in n - 6..n - 1 {
            for x in 0..80u16 {
                assert_eq!(Some(buf[(x, y as u16)].bg), raised, "({x}, {y})");
            }
        }
        assert!(rows[n - 1].contains("/ commands   @ files   shift+tab mode"), "{rows:#?}");
        assert_eq!(view.cursor, Some((4, (n - 5) as u16)), "the cursor at the text");
        let edge = |buf: &ratatui::buffer::Buffer, y: usize| buf[(0, y as u16)].fg;
        let default = edge(&buf, n - 5);
        a.status.mode = "plan".into();
        let (rows, buf) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows[n - 3].starts_with("▎   Plan  opus"));
        assert!(rows[n - 1].starts_with("  /p"), "the panel names the mode; the line keeps the folder");
        assert_ne!(edge(&buf, n - 5), default, "the edge takes the mode's colour");
        assert_eq!(Some(edge(&buf, n - 5)), a.theme.teal().fg);
        typed(&mut a, "fix it");
        let (rows, _) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows.last().unwrap().contains("enter send   shift+enter new line"));
    }

    #[test]
    fn the_panel_in_sixteen_colours_and_with_a_dialog() {
        let mut a = app(true);
        a.look = crate::tui::look::Look::Frame;
        // 16 colours: no grey behind the text (the nearest is too light), the edge still in the mode's colour.
        a.theme.depth = crate::tui::palette::Depth::Ansi16;
        let (rows, buf) = draw(&live_view(&a, 80, 12), 80);
        let y = rows.iter().position(|r| r.contains("What shall we build")).unwrap();
        assert_eq!(buf[(10, y as u16)].bg, ratatui::style::Color::Reset, "{rows:#?}");
        assert_eq!(Some(buf[(0, y as u16)].fg), a.theme.accent().fg);
        // While the model works: the panel says what typing does; the line, how to stop.
        a.busy = true;
        let (rows, _) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows.iter().any(|r| r.contains(QUEUE_PLACEHOLDER)), "{rows:#?}");
        assert!(rows.last().unwrap().contains("esc stop"));
        // A question open: it says its own keys, so the line names none.
        a.theme.depth = crate::tui::palette::Depth::True;
        ask(&mut a, "Bash", json!({"command": "cargo test"}));
        let (rows, _) = draw(&live_view(&a, 80, 30), 80);
        let last = rows.last().unwrap();
        assert!(!last.contains("esc stop") && !last.contains("queue"), "{last}");
    }

    #[test]
    fn a_long_model_name_never_hides_the_mode() {
        let mut a = app(true);
        a.look = crate::tui::look::Look::Frame;
        a.endpoint = "127.0.0.1:11434".into();
        a.status.model = "accounts/fireworks/models/a-very-long-model-name-that-goes-on-and-on".into();
        a.status.mode = "bypassPermissions".into();
        let (rows, _) = draw(&live_view(&a, 60, 12), 60);
        let meta = rows.iter().find(|r| r.contains("Bypass permissions")).expect("the mode stays");
        assert!(meta.contains("accounts/fireworks/") && meta.contains('…'), "the model shortened: {meta}");
        assert!(!meta.contains("127.0.0.1"), "the endpoint goes first: {meta}");
        assert!(rows.iter().all(|r| text_width(r) <= 60));
    }

    #[test]
    fn plan_mode_and_hints_show_in_the_status_line() {
        let mut a = app(true);
        a.status.mode = "plan".into();
        let (rows, _) = draw(&live_view(&a, 70, 10), 70);
        assert!(rows.last().unwrap().starts_with("  ‖ plan mode on (shift+tab to cycle)"));
        a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let (rows, _) = draw(&live_view(&a, 70, 10), 70);
        assert!(rows.last().unwrap().starts_with("  Press Ctrl-C again to exit"));
        let later = Instant::now() + HINT_FOR;
        let (rows, _) = draw(&live_view_at(&a, 70, 10, later), 70);
        assert!(rows.last().unwrap().starts_with("  ‖ plan mode"));
    }

    #[test]
    fn pickers_search_and_file_menus() {
        use kodact_core::commands::picker::{Choice, Pick, Picker};
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
        let mut a = App::new(
            Theme { color: false, light: false, accent: None, depth: Default::default() },
            vec!["cargo test".into(), "git status".into()],
        );
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        typed(&mut a, "car");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[0], "  search history: car");
        assert!(rows[1].starts_with("│   cargo test"), "the frame style's panel: {}", rows[1]);
        typed(&mut a, "zz");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[0], "  search history: carzz (no match)");

        // `@` lists matching paths.
        let mut a = app(false);
        a.on_event(UiEvent::Files(vec!["src/".into(), "src/main.rs".into(), "README.md".into()]));
        typed(&mut a, "look at @ma");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[3], "  src/main.rs");
    }

    fn diff_screen() -> kodact_core::commands::screens::Screen {
        let mut unified = String::from("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,30 @@\n-fn old() {}\n");
        for i in 0..30 {
            unified.push_str(&format!("+fn new_{i}() {{}}\n"));
        }
        unified.push_str("diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-Old title\n+New title\n");
        kodact_core::commands::screens::Screen {
            title: "Uncommitted changes (git diff HEAD)".into(),
            rows: kodact_core::commands::screens::diff_rows(&unified, &[], &[]),
        }
    }

    #[test]
    fn screens_scroll_inside_a_box_that_fits_the_terminal() {
        let mut a = app(false);
        a.on_event(UiEvent::Screen(diff_screen()));
        let n = a.viewer().unwrap().0.rows.len();
        let (rows, _) = draw(&live_view(&a, 40, 15), 40);
        assert!(rows.len() <= 15, "{rows:#?}");
        assert_eq!(rows[0], format!("╭{}╮", "─".repeat(38)));
        assert_eq!(rows[1], "│ Uncommitted changes (git diff HEAD)  │");
        assert_eq!(rows[2], "│ ❯ src/lib.rs  +30 -1                 │");
        assert_eq!(rows[3], "│   README.md  +1 -1                   │");
        let page = a.viewer_page.get();
        assert_eq!(page, 15 - 1 - 5, "the terminal minus the status line and the box");
        assert_eq!(rows[page + 3], format!("│ 1-{page} of {n} · ↑↓ PgUp PgDn · Enter op… │"));
        // Enter on a file jumps to its hunks; Esc goes back; scrolling keeps the cursor in view.
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let (rows, _) = draw(&live_view(&a, 40, 15), 40);
        assert_eq!(rows[2], "│ ❯ README.md                          │");
        assert_eq!(rows[3], "│   @@ -1 +1 @@                        │");
        assert_eq!(rows[4], "│   -Old title                         │", "no colour: the marker shows the side");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.viewer().unwrap().1, 1, "back on the file list");
        a.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        let (rows, _) = draw(&live_view(&a, 40, 15), 40);
        assert_eq!(rows[page + 1], "│ ❯ +New title                         │");
        assert!(rows[page + 3].contains(&format!("{}-{n} of {n}", n - page + 1)), "{}", rows[page + 3]);
        a.on_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(a.viewer().unwrap().1, n - 1 - page);
        a.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(a.viewer().unwrap().1, 0);
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(a.dialog.is_none(), "Esc at the top closes it");

        // A tiny terminal still shows the highlighted row.
        a.on_event(UiEvent::Screen(diff_screen()));
        let v = live_view(&a, 40, 4);
        assert!(v.lines.len() <= 4);

        // Colour at 100 columns: additions green, deletions red, hunk headers dim.
        let mut a = app(true);
        a.on_event(UiEvent::Screen(diff_screen()));
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let (rows, buf) = draw(&live_view(&a, 100, 40), 100);
        let y = rows.iter().position(|r| r.contains("-fn old() {}")).unwrap() as u16;
        assert_eq!(buf[(5, y)].fg, crate::tui::palette::DANGER.color(Default::default(), false));
        assert_eq!(buf[(5, y + 1)].fg, crate::tui::palette::SUCCESS.color(Default::default(), false));
        assert_eq!(
            buf[(5, y - 1)].fg,
            crate::tui::palette::MUTED.color(Default::default(), false),
            "{}",
            rows[y as usize - 1]
        );
        assert!(rows.iter().all(|r| r.chars().count() <= 100));
    }

    #[test]
    fn forms_show_fields_and_the_command_they_run() {
        use kodact_core::commands::screens::{Field, Form, Row, RowAction, Screen, Tone};
        let form = Form {
            title: "Add a hook".into(),
            fields: vec![Field::choice("Event", &["PreToolUse", "Stop"]), Field::text("Command", "")],
            template: "/hooks add {0} {1}".into(),
        };
        let mut a = app(false);
        a.on_event(UiEvent::Screen(Screen {
            title: "Hooks".into(),
            rows: vec![Row { action: Some(RowAction::Form(form)), ..Row::text("Add hook…", Tone::Plain) }],
        }));
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        a.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        a.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        typed(&mut a, "echo hi");
        let (rows, _) = draw(&live_view(&a, 60, 20), 60);
        assert_eq!(rows[1], format!("│ {:<56} │", "Add a hook"));
        assert_eq!(rows[2], format!("│ {:<56} │", "  Event    Stop"));
        assert_eq!(rows[3], format!("│ {:<56} │", "❯ Command  echo hi▏"));
        assert_eq!(rows[5], format!("│ {:<56} │", "Runs: /hooks add Stop 'echo hi'"));
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            vec![crate::tui::app::Action::Send("/hooks add Stop 'echo hi'".into())]
        );
        assert!(a.dialog.is_none());
    }

    #[test]
    fn context_parts_use_letters_without_colour() {
        use kodact_core::commands::screens::{Row, Screen, Span as S, Tone};
        let row = Row {
            spans: vec![
                S { text: "⛁ ".into(), tone: Tone::Part(0) },
                S { text: "⛁ ".into(), tone: Tone::Part(5) },
                S { text: "⛶ ".into(), tone: Tone::Part(6) },
            ],
            action: None,
        };
        let screen = Screen { title: "Context".into(), rows: vec![row] };
        let mut a = app(false);
        a.on_event(UiEvent::Screen(screen.clone()));
        let (rows, _) = draw(&live_view(&a, 40, 20), 40);
        assert_eq!(rows[2], format!("│ {:<36} │", "  S G ·"), "nothing to choose: no pointer");
        let mut a = app(true);
        a.on_event(UiEvent::Screen(screen));
        let (rows, buf) = draw(&live_view(&a, 40, 20), 40);
        assert_eq!(rows[2], format!("│ {:<36} │", "  ⛁ ⛁ ⛶"));
        let glyphs: Vec<u16> = (0..40).filter(|&x| buf[(x, 2)].symbol() == "⛁").collect();
        assert_eq!(buf[(glyphs[1], 2)].fg, crate::tui::palette::SUCCESS.color(Default::default(), false), "messages");
    }

    #[test]
    fn a_screen_with_nothing_to_choose_scrolls_by_page() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use kodact_core::commands::screens::{Row, Screen, Tone};
        let rows = (1..=30).map(|i| Row::text(format!("line {i}"), Tone::Plain)).collect();
        let mut a = app(false);
        a.on_event(UiEvent::Screen(Screen { title: "Info".into(), rows }));
        let first = draw(&live_view(&a, 40, 14), 40).0;
        let page = a.viewer_page.get();
        assert!(first[2].contains("  line 1 ") && !first.concat().contains('❯'), "{first:#?}");
        let key = |a: &mut App, c| a.on_term_event(crossterm::event::Event::Key(KeyEvent::new(c, KeyModifiers::NONE)));
        key(&mut a, KeyCode::Down);
        assert!(draw(&live_view(&a, 40, 14), 40).0[2].contains("line 2 "), "one row down");
        key(&mut a, KeyCode::End);
        let last = draw(&live_view(&a, 40, 14), 40).0;
        assert!(last[2].contains(&format!("line {} ", 31 - page)), "{last:#?}");
        key(&mut a, KeyCode::Esc);
        assert!(a.dialog.is_none());
    }

    #[test]
    fn a_running_tool_turns_its_dot_until_it_answers() {
        use kodact_types::{ApiMessage, ContentBlock, Role, StopReason, Usage};
        let mut a = app(true);
        a.on_event(UiEvent::Busy);
        let msg = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "src/a.rs"}),
                cache_control: None,
                provider: None,
            }],
            stop_reason: Some(StopReason::ToolUse),
            stop_sequence: None,
            usage: Usage::default(),
        };
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
            message: msg,
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        let since = a.running[0].since;
        let face = |at: Instant| {
            let (rows, buf) = draw(&live_view_at(&a, 60, 20, at), 60);
            let y = rows.iter().position(|r| r.contains("Reading src/a.rs")).expect("the call shows while it runs");
            (rows[y].chars().next().unwrap(), buf[(0, y as u16)].fg)
        };
        let (first, color) = face(since);
        let (later, _) = face(since + Duration::from_millis(250));
        assert_ne!(first, later, "the dot turns");
        assert!(glyphs::RUNNING.contains(&first.to_string().as_str()));
        assert_eq!(
            color,
            crate::tui::palette::BRAND.color(Default::default(), false),
            "running is lime, as the spinner"
        );
    }

    #[test]
    fn the_shimmer_sweeps_and_the_meter_warns() {
        let t = text::Theme { color: true, light: false, accent: None, depth: Default::default() };
        let at = |ms| shimmer(&t, "Acting…", Duration::from_millis(ms));
        // Where the brightest character is, in characters from the start.
        let bright = |spans: &[Span]| {
            let i = spans.iter().position(|s| s.style == t.shimmer(3))?;
            Some(spans[..i].iter().map(|s| s.content.chars().count()).sum::<usize>())
        };
        assert_ne!(bright(&at(500)), bright(&at(700)), "the highlight moves");
        let text: String = at(500).iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Acting…");
        assert_eq!(meter(&t, 50)[0].content, "━━━━");
        assert_eq!(meter(&t, 50)[0].style, t.accent());
        assert_eq!(meter(&t, 75)[0].style, t.warning());
        assert_eq!(meter(&t, 95)[0].style, t.error());
        assert_eq!(meter(&t, 0)[0].content, "");
    }

    #[test]
    fn the_full_screen_centres_the_front_then_keeps_the_input_at_the_bottom() {
        let mut a = app(false);
        a.front = true;
        a.endpoint = "127.0.0.1:11434".into();
        let page = FrontPage {
            above: vec![Line::from("WORDMARK"), Line::default(), Line::from("tagline"), Line::default()],
            below: vec![Line::default(), Line::from("/resume  checkout rounding")],
            box_left: 10,
            box_w: 80,
            under: true,
            ..Default::default()
        };
        let (v, _) = full_view(&a, 100, 30, &[], &page, 0);
        let (rows, _) = draw(&v, 100);
        assert_eq!(rows.len(), 30, "the whole screen");
        let mark = rows.iter().position(|r| r == "WORDMARK").unwrap();
        let resume = rows.iter().position(|r| r.contains("/resume")).unwrap();
        assert!(mark > 3 && resume < 25, "centred down the screen: {rows:#?}");
        // As many empty rows above the page as below it (the status line aside).
        let first = rows.iter().position(|r| !r.trim().is_empty()).unwrap();
        let last = rows[..29].iter().rposition(|r| !r.trim().is_empty()).unwrap();
        assert!(first.abs_diff(28 - last) <= 1, "centred: {first} above, {} below\n{rows:#?}", 28 - last);
        assert!(rows[mark + 3..resume].iter().any(|r| r.contains("❯ What shall we build today?")), "the input between");
        assert!(rows[29].ends_with(&format!("v{}", kodact_core::VERSION)), "the status line on the last row");
        let (_, cy) = v.cursor.unwrap();
        assert!(rows[cy as usize].contains('❯'), "the cursor in the input");
        // After the first message: the transcript from the top, the input at the bottom.
        a.submit("hello".into());
        let transcript: Vec<Line<'static>> = (1..=40).map(|i| Line::from(format!("line {i}"))).collect();
        let (v, max) = full_view(&a, 100, 30, &transcript, &page, 0);
        let (rows, _) = draw(&v, 100);
        assert_eq!(rows.len(), 30);
        assert!(rows.iter().any(|r| r == "line 40"), "the newest line shows");
        assert!(!rows.iter().any(|r| r == "line 1"));
        assert!(rows[25..].iter().any(|r| r.contains('❯')), "the input at the bottom: {rows:#?}");
        assert!(max > 0);
        // Scrolled up: older lines, and a note of what is below.
        let (v, _) = full_view(&a, 100, 30, &transcript, &page, 10);
        let (rows, _) = draw(&v, 100);
        // The last row turns into the note of what is below.
        assert!(rows.iter().any(|r| r == "line 29") && !rows.iter().any(|r| r == "line 31"), "{rows:#?}");
        assert!(rows.iter().any(|r| r.trim() == "↓ 10 more lines · PgDn"), "{rows:#?}");
        // A short transcript sits just above the live region, not at the top with a gap.
        let (v, _) = full_view(&a, 100, 30, &transcript[..2], &page, 0);
        let (rows, _) = draw(&v, 100);
        let at = rows.iter().position(|r| r == "line 2").unwrap();
        assert_eq!(rows[0], "");
        assert!(rows[at + 1..].iter().take(3).any(|r| r.contains('╭')), "{rows:#?}");
    }

    #[test]
    fn styles_change_the_bar_the_input_and_the_dialog() {
        use crate::tui::look::Look;
        let mut a = app(false);
        a.endpoint = "127.0.0.1:11434".into();
        a.branch = "main".into();
        // The status style puts a bar of readouts on top of the conversation.
        a.look = Look::Status;
        let page = FrontPage { box_left: 0, box_w: 100, ..Default::default() };
        let (v, _) = full_view(&a, 100, 20, &[Line::from("hello")], &page, 0);
        let (rows, _) = draw(&v, 100);
        assert!(rows[0].contains("model opus · 127.0.0.1:11434") && rows[0].contains("branch main"), "{}", rows[0]);
        a.status.cwd = "/tmp/a/very/long/path/that/goes/on/to/the/project/demo/proj2".into();
        let (v, _) = full_view(&a, 100, 20, &[Line::from("hello")], &page, 0);
        let bar = &draw(&v, 100).0[0];
        assert!(
            bar.contains("…") && bar.contains("proj2") && bar.contains("branch main"),
            "a long folder keeps its end: {bar}"
        );
        // Bare: the input under a rule, no box, a plain mark.
        a.look = Look::Bare;
        let (rows, _) = draw(&live_view(&a, 60, 10), 60);
        assert!(rows.iter().any(|r| r == &"─".repeat(60)));
        assert!(rows.iter().any(|r| r.starts_with("› What shall we build today?")), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains('╭')));
        // Frame: the question between corner marks.
        a.look = Look::Frame;
        ask(&mut a, "Bash", serde_json::json!({"command": "ls"}));
        let (rows, _) = draw(&live_view(&a, 60, 30), 60);
        assert!(rows[0].starts_with("┌─") && rows[0].ends_with("─┐"), "{rows:#?}");
        assert!(rows.iter().any(|r| r.starts_with("└─")));
    }

    #[test]
    fn the_front_screen_centres_its_input_until_something_is_sent() {
        let mut a = app(false);
        a.front = true;
        a.endpoint = "127.0.0.1:11434".into();
        let (rows, _) = draw(&live_view(&a, 100, 12), 100);
        let top = rows.iter().position(|r| r.contains('╭')).unwrap();
        assert_eq!(rows[top].find('╭'), Some(10), "an 80-column box, centred: {rows:#?}");
        assert!(rows[top + 1].trim_start().starts_with("│ ❯ What shall we build today?"));
        assert_eq!(
            rows[top + 3].trim(),
            "Default  ·  opus  ·  127.0.0.1:11434",
            "the mode, model and endpoint under it"
        );
        assert!(
            rows.last().unwrap().ends_with(&format!("v{}", kodact_core::VERSION)),
            "the version on the status line"
        );
        assert_eq!(live_view(&a, 100, 12).cursor, Some((14, top as u16 + 1)));
        // Sending anything ends the front screen: the box spans the terminal again.
        a.submit("hello".into());
        let (rows, _) = draw(&live_view(&a, 100, 12), 100);
        assert!(rows.iter().any(|r| r.starts_with('╭')), "{rows:#?}");
    }

    #[test]
    fn a_long_folder_keeps_its_end_and_leaves_room_for_the_model() {
        let mut a = app(false);
        a.status.cwd = "/tmp/a/very/long/path/that/goes/on/to/the/project/demo/proj2".into();
        let (rows, _) = draw(&live_view(&a, 60, 10), 60);
        let last = rows.last().unwrap();
        assert!(last.starts_with("  …") && last.contains("/demo/proj2"), "{last}");
        assert!(last.contains("opus"), "the model stays: {last}");
    }

    #[test]
    fn a_status_line_command_replaces_the_right_side() {
        let mut a = app(false);
        a.on_event(UiEvent::StatusLine(Some("main · 3 files".into())));
        let (rows, _) = draw(&live_view(&a, 40, 10), 40);
        assert_eq!(rows.last().unwrap(), "  /p                      main · 3 files");
    }
}
