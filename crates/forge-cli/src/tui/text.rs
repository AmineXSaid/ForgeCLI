//! Text for the terminal UI: the colour theme, a light markdown renderer
//! (one line at a time, so streamed text can be written as it arrives), and
//! wrapping of styled lines to the terminal's width.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Colours and emphasis. Without colour (`NO_COLOR`, `--color never`, the
/// `none` theme) only bold, dim and reverse are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub color: bool,
    /// Colours for a light background.
    pub light: bool,
    /// The accent colour chosen with `/color` for this session (`None`: Forge's own).
    pub accent: Option<Color>,
}

/// `/color` names and their colours.
pub const ACCENTS: &[(&str, Color)] = &[
    ("red", Color::Red),
    ("orange", Color::Rgb(0xe0, 0x8a, 0x3c)),
    ("yellow", Color::Yellow),
    ("green", Color::Green),
    ("cyan", Color::Cyan),
    ("blue", Color::Blue),
    ("purple", Color::Rgb(0x9b, 0x7b, 0xf0)),
    ("pink", Color::Rgb(0xe8, 0x7a, 0xb8)),
];

impl Theme {
    /// The theme named in settings (`dark`, `light`, `none`), when colour is allowed at all.
    pub fn named(name: &str, color_ok: bool) -> Theme {
        Theme { color: color_ok && name != "none", light: name == "light", accent: None }
    }

    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::default().fg(c)
        } else {
            Style::default()
        }
    }

    /// Forge's own colour: the prompt marker, the answer marker, selections.
    pub fn accent(&self) -> Style {
        if let (true, Some(c)) = (self.color, self.accent) {
            return Style::default().fg(c);
        }
        if self.color {
            Style::default().fg(if self.light { Color::Rgb(0x5b, 0x3c, 0xc4) } else { Color::Rgb(0x9b, 0x7b, 0xf0) })
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    pub fn dim(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn error(&self) -> Style {
        if self.color {
            self.fg(Color::Red)
        } else {
            self.bold()
        }
    }

    pub fn warning(&self) -> Style {
        self.fg(if self.light { Color::Rgb(0x9a, 0x6a, 0x00) } else { Color::Yellow })
    }

    pub fn success(&self) -> Style {
        self.fg(Color::Green)
    }

    /// A removed line in a diff.
    pub fn removed(&self) -> Style {
        if self.color {
            self.fg(if self.light { Color::Rgb(0xb0, 0x20, 0x20) } else { Color::Rgb(0xf0, 0x70, 0x70) })
        } else {
            Style::default()
        }
    }

    /// An added line in a diff.
    pub fn added(&self) -> Style {
        if self.color {
            self.fg(if self.light { Color::Rgb(0x1a, 0x7f, 0x37) } else { Color::Rgb(0x70, 0xd0, 0x80) })
        } else {
            Style::default()
        }
    }

    /// The colour of part `i` of the context window (`/context` grid).
    pub fn part(&self, i: u8) -> Style {
        if !self.color {
            return Style::default();
        }
        let c = match i {
            0 => Color::Rgb(0x9b, 0x7b, 0xf0),
            1 => Color::Cyan,
            2 => Color::Magenta,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Green,
            _ => return self.dim(),
        };
        Style::default().fg(c)
    }

    /// Inline `code`.
    pub fn code(&self) -> Style {
        if self.color {
            self.fg(if self.light { Color::Blue } else { Color::Cyan })
        } else {
            Style::default()
        }
    }

    /// The highlighted row of a menu or dialog.
    pub fn selected(&self) -> Style {
        if self.color {
            self.accent().add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        }
    }

    /// The person's own prompts in the transcript.
    pub fn user(&self) -> Style {
        if self.color && self.light {
            Style::default().bg(Color::Rgb(0xe6, 0xe6, 0xee)).fg(Color::Rgb(0x10, 0x10, 0x18))
        } else if self.color {
            Style::default().bg(Color::Rgb(0x3a, 0x3a, 0x44)).fg(Color::Rgb(0xe8, 0xe8, 0xf0))
        } else {
            self.dim()
        }
    }
}

/// Markdown, one line at a time: code fences, headings, `**bold**`, `` `code` ``
/// and tables. Table rows are held until the table ends, so its columns line up.
#[derive(Debug, Clone, Default)]
pub struct Markdown {
    in_fence: bool,
    table: Vec<String>,
}

/// A table row: `| a | b |`.
fn is_table_row(s: &str) -> bool {
    let t = s.trim();
    t.starts_with('|') && t.len() > 1
}

fn cells(row: &str) -> Vec<String> {
    let t = row.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

impl Markdown {
    /// The lines `raw` becomes: none while a table or a fence marker is being
    /// read, the finished table when a table ends, else one line.
    pub fn push(&mut self, raw: &str, t: &Theme) -> Vec<Line<'static>> {
        if !self.in_fence && is_table_row(raw) {
            self.table.push(raw.to_string());
            return vec![];
        }
        let mut out = self.finish(t);
        if let Some(l) = self.line(raw, t) {
            out.push(l);
        }
        out
    }

    /// A table still being held, drawn now (at the end of a message).
    pub fn finish(&mut self, t: &Theme) -> Vec<Line<'static>> {
        if self.table.is_empty() {
            return vec![];
        }
        let rows: Vec<Vec<String>> = std::mem::take(&mut self.table).iter().map(|r| cells(r)).collect();
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; n];
        for r in rows.iter().filter(|r| !is_separator(r)) {
            for (i, c) in r.iter().enumerate() {
                let plain: String = inline(c, t).iter().map(|s| s.content.as_ref()).collect();
                widths[i] = widths[i].max(width(&plain));
            }
        }
        let bar = || Span::styled(" │ ", t.dim());
        let mut out = vec![];
        for (ri, r) in rows.iter().enumerate() {
            if is_separator(r) {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                out.push(Line::from(Span::styled(rule.join("─┼─"), t.dim())));
                continue;
            }
            let header = ri == 0 && rows.get(1).is_some_and(|r| is_separator(r));
            let mut spans = vec![];
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    spans.push(bar());
                }
                let c = r.get(i).map(String::as_str).unwrap_or("");
                let mut cell = inline(c, t);
                if header {
                    for s in &mut cell {
                        s.style = s.style.patch(t.bold());
                    }
                }
                let used: usize = cell.iter().map(|s| width(&s.content)).sum();
                spans.extend(cell);
                if i + 1 < widths.len() {
                    spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
                }
            }
            out.push(Line::from(spans));
        }
        out
    }

    /// One line, or none for a fence marker (code shows as indented, coloured
    /// lines; the backticks aren't drawn).
    pub fn line(&mut self, raw: &str, t: &Theme) -> Option<Line<'static>> {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            self.in_fence = !self.in_fence;
            return None;
        }
        if self.in_fence {
            return Some(Line::from(Span::styled(format!("  {raw}"), t.code())));
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let text = rest.trim_start_matches('#').trim();
            return Some(Line::from(Span::styled(text.to_string(), t.bold())));
        }
        Some(Line::from(inline(raw, t)))
    }

    pub fn reset(&mut self) {
        self.in_fence = false;
        self.table.clear();
    }
}

/// `**bold**` and `` `code` `` inside a line.
pub fn inline(s: &str, t: &Theme) -> Vec<Span<'static>> {
    let mut spans = vec![];
    let mut plain = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        let (marker, style) = if rest.starts_with("**") {
            ("**", t.bold())
        } else if rest.starts_with('`') {
            ("`", t.code())
        } else {
            let c = rest.chars().next().unwrap();
            plain.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        let body = &rest[marker.len()..];
        match body.find(marker) {
            Some(end) if end > 0 => {
                if !plain.is_empty() {
                    spans.push(Span::raw(std::mem::take(&mut plain)));
                }
                // Without colour, code keeps its backticks: nothing else sets it apart.
                let text = match marker {
                    "`" if !t.color => format!("`{}`", &body[..end]),
                    _ => body[..end].to_string(),
                };
                spans.push(Span::styled(text, style));
                rest = &body[end + marker.len()..];
            }
            _ => {
                plain.push_str(marker);
                rest = body;
            }
        }
    }
    if !plain.is_empty() || spans.is_empty() {
        spans.push(Span::raw(plain));
    }
    spans
}

/// Display width of a string.
pub fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Wrap styled lines to `width` columns, breaking at spaces where it can.
/// The indent a wrapped row continues at: the line's leading spaces plus a
/// leading marker (`• `, `› `, `↳ `, `- `, `* `, `> `, `12. `), so lists,
/// answers and tool results wrap under their text, not at column 0.
fn hanging_indent(cells: &[(char, Style)], max: usize) -> usize {
    let text: String = cells.iter().map(|(c, _)| *c).collect();
    let lead = text.len() - text.trim_start_matches(' ').len();
    let rest = &text[lead..];
    let marker = ["• ", "› ", "↳ ", "- ", "* ", "> ", "‖ ", "» "]
        .iter()
        .find(|m| rest.starts_with(**m))
        .map(|m| m.chars().count())
        .or_else(|| {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            (digits > 0 && digits < 4 && rest[digits..].starts_with(". ")).then_some(digits + 2)
        })
        .unwrap_or(0);
    // Spaces after the marker belong to it too (`↳  text`).
    let after = rest.chars().skip(marker).take_while(|c| *c == ' ').count();
    let start = lead + marker + if marker > 0 { after } else { 0 };
    // Columns (`Ctrl+R      Search earlier prompts`, `Model:   x`, `ok   git   found`):
    // continue under the last one that starts near the left, so a wrapped
    // value doesn't look like a key. Columns are set apart by 2+ spaces.
    let chars: Vec<char> = text.chars().collect();
    let limit = start + COLUMN_LIMIT.min(max.saturating_sub(start));
    let mut column = None;
    let mut i = start;
    while i < chars.len() && i < limit {
        if chars[i] != ' ' {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j - i >= 2 && j < chars.len() && j <= limit {
            column = Some(j);
        }
        i = j;
    }
    width(&chars[..column.unwrap_or(start).min(chars.len())].iter().collect::<String>())
}

/// How far from the text's start a value column may begin and still be used as the indent.
const COLUMN_LIMIT: usize = 28;

pub fn wrap(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = (width as usize).max(1);
    let mut out = vec![];
    for line in lines {
        let base = line.style;
        let cells: Vec<(char, Style)> =
            line.spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, base.patch(s.style)))).collect();
        if cells.is_empty() {
            out.push(Line::default());
            continue;
        }
        // Continuation rows start under the text; never more than half the width.
        let indent = hanging_indent(&cells, width / 2).min(width / 2);
        let mut start = 0;
        let mut first = true;
        while start < cells.len() {
            let room = if first { width } else { width - indent };
            let width = room.max(1);
            let mut w = 0;
            let mut end = start;
            let mut last_space = None;
            while end < cells.len() {
                let cw = cells[end].0.width().unwrap_or(0);
                if w + cw > width && end > start {
                    break;
                }
                if cells[end].0 == ' ' {
                    last_space = Some(end);
                }
                w += cw;
                end += 1;
            }
            let mut next = end;
            if end < cells.len() {
                // On the first row, a break inside the padding before the value
                // column would leave a bare label: break the long value instead.
                let label = |sp: usize| first && width_of(&cells[start..sp]) <= indent;
                if let Some(sp) = last_space.filter(|sp| *sp > start && !label(*sp)) {
                    // Break after the space; the space ends this row.
                    end = sp;
                    next = sp + 1;
                }
            }
            let mut row = spans_of(&cells[start..end]);
            if !first && indent > 0 {
                row.spans.insert(0, Span::raw(" ".repeat(indent)));
            }
            out.push(row);
            first = false;
            start = next;
            // A continuation row doesn't start with the space it broke at.
            while start < cells.len() && cells[start].0 == ' ' && indent > 0 {
                start += 1;
            }
        }
    }
    out
}

fn width_of(cells: &[(char, Style)]) -> usize {
    cells.iter().map(|(c, _)| c.width().unwrap_or(0)).sum()
}

fn spans_of(cells: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![];
    for (c, st) in cells {
        match spans.last_mut() {
            Some(s) if s.style == *st => s.content.to_mut().push(*c),
            _ => spans.push(Span::styled(c.to_string(), *st)),
        }
    }
    Line::from(spans)
}

/// A line's text without styles (tests).
#[cfg(test)]
pub fn plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Theme = Theme { color: true, light: false, accent: None };

    #[test]
    fn markdown_lines_keep_track_of_fences() {
        let mut md = Markdown::default();
        assert_eq!(plain(&md.line("## Plan", &T).unwrap()), "Plan");
        let l = md.line("Run `cargo test` and **check** it", &T).unwrap();
        assert_eq!(plain(&l), "Run cargo test and check it");
        assert_eq!(l.spans[1].style, T.code());
        assert_eq!(l.spans[3].style, T.bold());
        let mono = Theme { color: false, ..T };
        let l = Markdown::default().line("Run `cargo test` and **check** it", &mono).unwrap();
        assert_eq!(plain(&l), "Run `cargo test` and check it", "without colour, code keeps its backticks");
        assert!(md.line("```rust", &T).is_none(), "fence markers aren't drawn");
        assert_eq!(plain(&md.line("# not a heading", &T).unwrap()), "  # not a heading");
        md.line("```", &T);
        assert_eq!(plain(&md.line("# heading", &T).unwrap()), "heading");
        assert_eq!(plain(&md.line("a ` lone backtick", &T).unwrap()), "a ` lone backtick");
    }

    #[test]
    fn tables_line_up_when_they_end() {
        let mut md = Markdown::default();
        assert!(md.push("| Check | Result |", &T).is_empty());
        assert!(md.push("| --- | --- |", &T).is_empty());
        assert!(md.push("| `cargo test` | 12 passed |", &T).is_empty());
        let out: Vec<String> = md.push("After.", &T).iter().map(plain).collect();
        assert_eq!(out, ["Check      │ Result", "───────────┼──────────", "cargo test │ 12 passed", "After."]);
        assert!(md.push("| a | b |", &T).is_empty());
        assert_eq!(md.finish(&T).len(), 1, "a table at the end of a message is drawn by finish");
    }

    #[test]
    fn wraps_at_spaces_and_by_display_width() {
        let rows: Vec<String> = wrap(vec![Line::from("the quick brown fox jumps")], 10).iter().map(plain).collect();
        assert_eq!(rows, ["the quick", "brown fox", "jumps"]);
        let rows: Vec<String> = wrap(vec![Line::from("abcdefghijkl")], 5).iter().map(plain).collect();
        assert_eq!(rows, ["abcde", "fghij", "kl"]);
        // Wide characters take two columns.
        let rows: Vec<String> = wrap(vec![Line::from("日本語のテキスト")], 6).iter().map(plain).collect();
        assert_eq!(rows, ["日本語", "のテキ", "スト"]);
        assert_eq!(wrap(vec![Line::default()], 5).len(), 1);
        // Lists, answers and results wrap under their text.
        let rows: Vec<String> = wrap(vec![Line::from("  - one two three four")], 12).iter().map(plain).collect();
        assert_eq!(rows, ["  - one two", "    three", "    four"]);
        let rows: Vec<String> = wrap(vec![Line::from("  ↳  alpha beta gamma")], 14).iter().map(plain).collect();
        assert_eq!(rows, ["  ↳  alpha", "     beta", "     gamma"]);
        let rows: Vec<String> = wrap(vec![Line::from("12. aaaa bbbb")], 9).iter().map(plain).collect();
        assert_eq!(rows, ["12. aaaa", "    bbbb"]);
        // Two columns: the description wraps under itself.
        let rows: Vec<String> =
            wrap(vec![Line::from("Ctrl+C    clear the input or exit")], 24).iter().map(plain).collect();
        assert_eq!(rows, ["Ctrl+C    clear the", "          input or exit"]);
        // A doctor row continues under its detail; a long path breaks in place.
        let rows: Vec<String> =
            wrap(vec![Line::from("FAIL session      no key was sent anywhere")], 40).iter().map(plain).collect();
        assert_eq!(rows, ["FAIL session      no key was sent", "                  anywhere"]);
        let rows: Vec<String> =
            wrap(vec![Line::from("Directory:   /tmp/a-long/path/to/proj")], 30).iter().map(plain).collect();
        assert_eq!(rows, ["Directory:   /tmp/a-long/path/", "             to/proj"]);
        // Styles survive the wrap.
        let l = Line::from(vec![Span::raw("aa "), Span::styled("bbbb", T.bold())]);
        let w = wrap(vec![l], 4);
        assert_eq!(w[1].spans[0].style, T.bold());
    }
}
