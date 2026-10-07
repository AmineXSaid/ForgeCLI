//! Text for the terminal UI: the colour theme, a light markdown renderer
//! (one line at a time, so streamed text can be written as it arrives), and
//! wrapping of styled lines to the terminal's width.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Colours and emphasis. Without colour (`NO_COLOR`, `--color never`) only
/// bold, dim and reverse are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub color: bool,
}

impl Theme {
    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::default().fg(c)
        } else {
            Style::default()
        }
    }

    /// Forge's own colour: the prompt marker, the answer marker, selections.
    pub fn accent(&self) -> Style {
        if self.color {
            Style::default().fg(Color::Rgb(0x9b, 0x7b, 0xf0))
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
        self.fg(Color::Yellow)
    }

    pub fn success(&self) -> Style {
        self.fg(Color::Green)
    }

    /// Inline `code`.
    pub fn code(&self) -> Style {
        if self.color {
            self.fg(Color::Cyan)
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
        if self.color {
            Style::default().bg(Color::Rgb(0x3a, 0x3a, 0x44)).fg(Color::Rgb(0xe8, 0xe8, 0xf0))
        } else {
            self.dim()
        }
    }
}

/// Markdown, one line at a time: code fences, headings, `**bold**` and `` `code` ``.
#[derive(Debug, Clone, Default)]
pub struct Markdown {
    in_fence: bool,
}

impl Markdown {
    pub fn line(&mut self, raw: &str, t: &Theme) -> Line<'static> {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            self.in_fence = !self.in_fence;
            return Line::from(Span::styled(raw.to_string(), t.dim()));
        }
        if self.in_fence {
            return Line::from(Span::styled(format!("  {raw}"), t.code()));
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let text = rest.trim_start_matches('#').trim();
            return Line::from(Span::styled(text.to_string(), t.bold()));
        }
        Line::from(inline(raw, t))
    }

    pub fn reset(&mut self) {
        self.in_fence = false;
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
                spans.push(Span::styled(body[..end].to_string(), style));
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
        let mut start = 0;
        while start < cells.len() {
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
                if let Some(sp) = last_space.filter(|sp| *sp > start) {
                    // Break after the space; the space ends this row.
                    end = sp;
                    next = sp + 1;
                }
            }
            out.push(spans_of(&cells[start..end]));
            start = next;
        }
    }
    out
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

/// A line's text without styles (tests, copying).
#[cfg(test)]
pub fn plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Theme = Theme { color: true };

    #[test]
    fn markdown_lines_keep_track_of_fences() {
        let mut md = Markdown::default();
        assert_eq!(plain(&md.line("## Plan", &T)), "Plan");
        let l = md.line("Run `cargo test` and **check** it", &T);
        assert_eq!(plain(&l), "Run cargo test and check it");
        assert_eq!(l.spans[1].style, T.code());
        assert_eq!(l.spans[3].style, T.bold());
        md.line("```rust", &T);
        assert_eq!(plain(&md.line("# not a heading", &T)), "  # not a heading");
        md.line("```", &T);
        assert_eq!(plain(&md.line("# heading", &T)), "heading");
        assert_eq!(plain(&md.line("a ` lone backtick", &T)), "a ` lone backtick");
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
        // Styles survive the wrap.
        let l = Line::from(vec![Span::raw("aa "), Span::styled("bbbb", T.bold())]);
        let w = wrap(vec![l], 4);
        assert_eq!(w[1].spans[0].style, T.bold());
    }
}
