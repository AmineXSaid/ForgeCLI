//! Forge's mark in the terminal: the F-and-cube logo, drawn as a pixel sprite
//! in half blocks (each cell is two pixels stacked, so the pixels come out
//! square), and the two screens it heads: the welcome banner and the
//! first-run card. The sprite and its inks are Forge for VS Code's
//! (`terminalBrand.ts`), in its Pajamas purples.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::palette::{self, Depth};
use super::text::{width, Theme};

/// The logo, one character per pixel, lit from the top left; `.` is empty.
///
/// `O` outline, `H` the F's lit edge, `F` the F, `D` its shaded edge, `T` the
/// cube's top, `G` a glint on it, `L` the cube's lit face, `S` its shaded
/// face, `E` the edge between them.
pub const MARK: [&str; 16] = [
    "OOOOOOOOOOOOOOOO",
    "OHHHHHHHHHHHHHHO",
    "OHFFFFFFFFFFFFDO",
    "OHFFFFFFFFFFFFDO",
    "OHFFFDDDDDDDDDDO",
    "OHFFDOOOOOOOOOOO",
    "OHFFDO....OO....",
    "OHFFDO..OOTTOO..",
    "OHFFDOOOTTGTTTOO",
    "OHFFDOLLTTTTTSSO",
    "OHFFDOLLLLTSSSSO",
    "OHFFDOLLLLESSSSO",
    "OHFFDOLLLLESSSSO",
    "OHFFDOOLLLESSSOO",
    "ODDDDO.OOLESOO..",
    "OOOOOO...OOO....",
];

/// The 12-pixel cut, for narrow terminals.
pub const MARK_SMALL: [&str; 12] = [
    "OOOOOOOOOOOO",
    "OHHHHHHHHHHO",
    "OHFFFFFFFFDO",
    "OHFFDDDDDDDO",
    "OHFDOOOOOOOO",
    "OHFDO..OO...",
    "OHFDO.OTTO..",
    "OHFDOOTGTTOO",
    "OHFDOLLTTSSO",
    "OHFDOLLLESSO",
    "ODDDOOLLESOO",
    "OOOOO.OOOO..",
];

/// The logo's inks: hex, and the ANSI colour for 16-colour terminals.
fn mark_ink(c: char) -> Option<(u32, Color)> {
    Some(match c {
        'O' => (0x27243e, Color::Reset),
        'H' => (0xac93e6, Color::LightMagenta),
        'F' => (0x7b58cf, Color::Magenta),
        'D' => (0x5c47a6, Color::Magenta),
        'T' => (0xe1d8f9, Color::White),
        'G' => (0xf4f0ff, Color::White),
        'L' => (0x9475db, Color::LightMagenta),
        'S' => (0x493c83, Color::Magenta),
        'E' => (0x342d59, Color::Magenta),
        _ => return None,
    })
}

/// A sprite in half blocks. The outline is drawn only where it reads: on a
/// light background, in 256 or 24-bit colour. Without colour the shape is
/// drawn in the terminal's own colour.
fn sprite(rows: &[&str], ink: fn(char) -> Option<(u32, Color)>, t: &Theme) -> Vec<Line<'static>> {
    let outline = t.color && t.light && t.depth != Depth::Ansi16;
    let color_of = |c: Option<char>| -> Option<Color> {
        let c = c?;
        if c == 'O' && !outline {
            return None;
        }
        let (hex, ansi) = ink(c)?;
        if !t.color {
            return Some(Color::Reset);
        }
        if t.depth == Depth::Ansi16 && ansi == Color::Reset {
            return None;
        }
        Some(palette::ink(hex, t.depth, ansi))
    };
    let mut out = vec![];
    for pair in rows.chunks(2) {
        let upper: Vec<char> = pair[0].chars().collect();
        let lower: Vec<char> = pair.get(1).map(|r| r.chars().collect()).unwrap_or_default();
        let mut spans: Vec<Span<'static>> = vec![];
        for x in 0..upper.len() {
            let (u, l) = (color_of(upper.get(x).copied()), color_of(lower.get(x).copied()));
            let (text, style) = match (u, l) {
                (None, None) => (" ", Style::default()),
                (Some(u), None) => ("▀", fg(u)),
                (None, Some(l)) => ("▄", fg(l)),
                (Some(u), Some(l)) if u == l => ("█", fg(u)),
                (Some(u), Some(l)) => ("▀", fg(u).bg(l)),
            };
            match spans.last_mut() {
                Some(s) if s.style == style => s.content.to_mut().push_str(text),
                _ => spans.push(Span::styled(text.to_string(), style)),
            }
        }
        out.push(Line::from(spans));
    }
    out
}

fn fg(c: Color) -> Style {
    if c == Color::Reset {
        Style::default()
    } else {
        Style::default().fg(c)
    }
}

/// The logo: 16 columns by 8 rows, or 12 by 6 when `small`.
pub fn mark(t: &Theme, small: bool) -> Vec<Line<'static>> {
    if small {
        sprite(&MARK_SMALL, mark_ink, t)
    } else {
        sprite(&MARK, mark_ink, t)
    }
}

/// The logo at text size, for one-line places: `▛◆`.
pub fn mark_inline(t: &Theme) -> Vec<Span<'static>> {
    let (f, cube) = if t.color {
        (
            Style::default().fg(palette::ink(0x7759c2, t.depth, Color::Magenta)),
            Style::default().fg(palette::ink(0xcbbbf2, t.depth, Color::LightMagenta)),
        )
    } else {
        (Style::default(), Style::default())
    };
    vec![Span::styled("▛", f), Span::styled("◆", cube)]
}

/// Art beside text, top-aligned, with a two-column margin and a three-column gap.
fn beside(art: Vec<Line<'static>>, art_w: usize, text: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let rows = art.len().max(text.len());
    let mut art = art.into_iter();
    let mut text = text.into_iter();
    (0..rows)
        .map(|_| {
            let mut spans = vec![Span::raw("  ")];
            match art.next() {
                Some(a) => {
                    let w: usize = a.spans.iter().map(|s| width(&s.content)).sum();
                    spans.extend(a.spans);
                    spans.push(Span::raw(" ".repeat(art_w.saturating_sub(w) + 3)));
                }
                None => spans.push(Span::raw(" ".repeat(art_w + 3))),
            }
            if let Some(l) = text.next() {
                spans.extend(l.spans);
            }
            trim_end(Line::from(spans))
        })
        .collect()
}

fn trim_end(mut l: Line<'static>) -> Line<'static> {
    while l.spans.last().is_some_and(|s| s.content.trim().is_empty() && s.style.bg.is_none()) {
        l.spans.pop();
    }
    l
}

/// What the welcome banner says.
pub struct Welcome<'a> {
    pub version: &'a str,
    pub model: &'a str,
    /// Where requests go: a host, or the kind of endpoint.
    pub endpoint: &'a str,
    /// The working directory, as people write it (`~/proj`).
    pub cwd: &'a str,
}

/// The keys worth knowing on the first screen, key first.
pub const KEYS: [(&str, &str); 4] = [("/", "commands"), ("@", "files"), ("!", "shell"), ("?", "shortcuts")];

/// The line under the banner, as Forge's welcome page says it.
pub const WELCOME_LINE: &str = "What to do first? Ask about this codebase or we can start writing code.";

fn keys_line(t: &Theme, gap: usize) -> Line<'static> {
    let mut spans = vec![];
    for (i, (k, what)) in KEYS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled(k.to_string(), t.accent().add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {what}"), t.dim()));
    }
    Line::from(spans)
}

/// The welcome banner printed once at the top of a session: the logo beside
/// ForgeCLI and its version, the model and where it runs, the directory and
/// the keys to know; then a rule and the welcome line. `cols` is the
/// terminal's width; narrow terminals get the small logo, or none.
pub fn welcome(t: &Theme, w: &Welcome, cols: usize) -> Vec<Line<'static>> {
    let title = Line::from(vec![
        Span::styled("ForgeCLI", t.brand_strong().add_modifier(Modifier::BOLD)),
        Span::styled(format!("  v{}", w.version), t.dim()),
    ]);
    let runs = Line::from(vec![
        Span::styled(w.model.to_string(), t.bold()),
        Span::styled(" on ", t.dim()),
        Span::raw(w.endpoint.to_string()),
    ]);
    let cwd = Line::from(Span::styled(w.cwd.to_string(), t.dim()));
    let mut out = vec![Line::default()];
    let text_w = [width(w.model) + 4 + width(w.endpoint), width(w.cwd), 41].into_iter().max().unwrap_or(0);
    if cols >= 16 + 5 + text_w.min(56) && cols >= 64 {
        let text = vec![Line::default(), title, runs, cwd, Line::default(), keys_line(t, 3)];
        out.extend(beside(mark(t, false), 16, text));
    } else if cols >= 12 + 5 + 41 {
        let text = vec![title, runs, cwd, Line::default(), keys_line(t, 2)];
        out.extend(beside(mark(t, true), 12, text));
    } else {
        for l in [title, runs, cwd] {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(l.spans);
            out.push(Line::from(spans));
        }
    }
    out.push(Line::from(vec![Span::raw("  "), Span::styled("─".repeat(cols.saturating_sub(4).min(76)), t.hairline())]));
    out.push(Line::from(vec![Span::raw("  "), Span::styled(WELCOME_LINE, t.dim())]));
    super::text::wrap(out, cols as u16)
}

/// Ways to give Forge an endpoint, for the first-run card: what to set, then why.
fn setup_lines(t: &Theme) -> Vec<Line<'static>> {
    let cmd = |var: &str, value: &str| {
        if cfg!(windows) {
            format!("$env:{var}=\"{value}\"")
        } else {
            format!("export {var}={value}")
        }
    };
    let rows: [(&str, &str, &str); 5] = [
        ("FORGE_OPENAI_BASE_URL", "http://localhost:11434/v1", "Ollama; LM Studio is :1234/v1"),
        ("FORGE_OPENAI_API_KEY", "<key>", "only if it asks for one"),
        ("FORGE_MODEL", "<model>", "one the server lists"),
        ("FORGE_API_KEY", "<key>", ""),
        ("FORGE_BASE_URL", "https://gateway.example.com", "for a gateway"),
    ];
    // The notes line up in one column.
    let col = rows.iter().map(|(v, x, _)| width(&cmd(v, x))).max().unwrap_or(0) + 3;
    let set = |(var, value, note): (&str, &str, &str)| -> Line<'static> {
        let c = cmd(var, value);
        let mut spans = vec![Span::raw("    "), Span::styled("$ ", t.accent()), Span::styled(c.clone(), t.code())];
        if !note.is_empty() {
            spans.push(Span::raw(" ".repeat(col - width(&c))));
            spans.push(Span::styled(note.to_string(), t.subtle()));
        }
        Line::from(spans)
    };
    let head = |s: &str| Line::from(vec![Span::raw("  "), Span::styled(s.to_string(), t.bold())]);
    let mut out = vec![head("A local server or an OpenAI-compatible gateway")];
    out.extend(rows[..3].iter().map(|r| set(*r)));
    out.push(Line::default());
    out.push(head("A Messages API endpoint"));
    out.extend(rows[3..].iter().map(|r| set(*r)));
    out
}

/// The card's text beside the F logo, or alone when the terminal is narrow.
fn text_beside_mark(t: &Theme, cols: usize, text: Vec<Line<'static>>) -> Vec<Line<'static>> {
    if cols >= 16 + 5 + 40 {
        return beside(mark(t, false), 16, text);
    }
    text.into_iter()
        .map(|l| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(l.spans);
            Line::from(spans)
        })
        .collect()
}

/// The first-run card, shown instead of a session when no endpoint is set
/// up: the F logo beside what ForgeCLI is, the servers it works with, how to
/// set one up, and what Forge found.
pub fn first_run(t: &Theme, version: &str, problem: &str, cols: usize) -> Vec<Line<'static>> {
    let eyebrow = Line::from(Span::styled(format!("FORGECLI  v{version}"), t.dim().add_modifier(Modifier::BOLD)));
    let headline = Line::from(vec![
        Span::styled("A coding agent, ", t.bold()),
        Span::styled("reforged", t.brand_strong().add_modifier(Modifier::BOLD)),
        Span::styled(",", t.bold()),
    ]);
    let headline2 = Line::from(Span::styled("on the model you choose.", t.bold()));
    let lede = Line::from(Span::styled("Set up an endpoint and run forge again.", t.dim()));
    let mut out = vec![Line::default()];
    let text = vec![Line::default(), eyebrow, Line::default(), headline, headline2, Line::default(), lede];
    out.extend(text_beside_mark(t, cols, text));
    out.push(Line::default());
    // Forge's runtime statement: the names stand out, the rest is quiet.
    let mut runtimes = vec![Span::raw("  ")];
    for (i, name) in ["Ollama", "LM Studio", "vLLM", "llama.cpp"].iter().enumerate() {
        if i > 0 {
            runtimes.push(Span::styled(", ", t.dim()));
        }
        runtimes.push(Span::styled(name.to_string(), t.bold()));
    }
    runtimes.push(Span::styled(" and ", t.dim()));
    runtimes.push(Span::styled("Jan", t.bold()));
    runtimes.push(Span::styled(", or any ", t.dim()));
    runtimes.push(Span::styled("OpenAI", t.bold()));
    runtimes.push(Span::styled("- or ", t.dim()));
    runtimes.push(Span::styled("Messages", t.bold()));
    runtimes.push(Span::styled("-compatible gateway.", t.dim()));
    out.push(Line::from(runtimes));
    out.push(Line::default());
    out.extend(setup_lines(t));
    out.push(Line::default());
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Forge found  ", t.warning()),
        Span::styled(problem.to_string(), t.dim()),
    ]));
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("forge doctor --probe", t.code()),
        Span::styled(" checks what is set and asks the endpoint once.", t.dim()),
    ]));
    out.push(Line::default());
    super::text::wrap(out, cols as u16)
}

/// A line as text with ANSI styles, for printing before (or without) the UI.
pub fn ansi(line: &Line) -> String {
    let mut s = String::new();
    for span in &line.spans {
        let st = line.style.patch(span.style);
        let mut codes: Vec<String> = vec![];
        if st.add_modifier.contains(Modifier::BOLD) {
            codes.push("1".into());
        }
        if st.add_modifier.contains(Modifier::DIM) {
            codes.push("2".into());
        }
        if st.add_modifier.contains(Modifier::REVERSED) {
            codes.push("7".into());
        }
        if let Some(c) = st.fg.and_then(|c| sgr(c, false)) {
            codes.push(c);
        }
        if let Some(c) = st.bg.and_then(|c| sgr(c, true)) {
            codes.push(c);
        }
        if codes.is_empty() {
            s.push_str(&span.content);
        } else {
            s.push_str(&format!("\x1b[{}m{}\x1b[0m", codes.join(";"), span.content));
        }
    }
    s
}

fn sgr(c: Color, bg: bool) -> Option<String> {
    let base = if bg { 40 } else { 30 };
    let named = |i: u8| Some((base + i).to_string());
    let bright = |i: u8| Some((base + 60 + i).to_string());
    match c {
        Color::Reset => None,
        Color::Black => named(0),
        Color::Red => named(1),
        Color::Green => named(2),
        Color::Yellow => named(3),
        Color::Blue => named(4),
        Color::Magenta => named(5),
        Color::Cyan => named(6),
        Color::Gray => named(7),
        Color::DarkGray => bright(0),
        Color::LightRed => bright(1),
        Color::LightGreen => bright(2),
        Color::LightYellow => bright(3),
        Color::LightBlue => bright(4),
        Color::LightMagenta => bright(5),
        Color::LightCyan => bright(6),
        Color::White => bright(7),
        Color::Indexed(n) => Some(format!("{};5;{n}", if bg { 48 } else { 38 })),
        Color::Rgb(r, g, b) => Some(format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect()
    }

    const DARK: Theme = Theme { color: true, light: false, accent: None, depth: Depth::True };

    #[test]
    fn sprites_are_square_pixels_in_half_blocks() {
        for rows in [&MARK[..], &MARK_SMALL[..]] {
            assert!(rows.iter().all(|r| r.chars().count() == rows.len()), "square");
        }
        let m = mark(&DARK, false);
        assert_eq!(m.len(), 8);
        assert!(m.iter().all(|l| l.spans.iter().map(|s| width(&s.content)).sum::<usize>() <= 16));
        assert_eq!(mark(&DARK, true).len(), 6);
        // The F's top bar is a full row of upper/lower halves in brand purples.
        let top = &m[1];
        assert!(top.spans.iter().any(|s| s.style.fg == Some(Color::Rgb(0x7b, 0x58, 0xcf))), "{top:?}");
    }

    #[test]
    fn without_colour_the_mark_keeps_its_shape() {
        let mono = Theme { color: false, ..DARK };
        let m = plain(&mark(&mono, false));
        assert!(m[1].starts_with(" ▀▀▀▀") || m[1].starts_with(" ███"), "{m:?}");
        assert!(mark(&mono, false)
            .iter()
            .all(|l| l.spans.iter().all(|s| s.style.fg.is_none() && s.style.bg.is_none())));
    }

    #[test]
    fn the_welcome_banner_fits_the_width() {
        let w = Welcome { version: "0.1.0", model: "deep-thinking", endpoint: "localhost:11434", cwd: "~/proj" };
        for cols in [120, 80, 60, 40] {
            let lines = welcome(&DARK, &w, cols);
            for l in &lines {
                let lw: usize = l.spans.iter().map(|s| width(&s.content)).sum();
                assert!(lw <= cols, "{cols}: {lw} {:?}", plain(std::slice::from_ref(l)));
            }
            let text = plain(&lines).join("\n");
            assert!(text.contains("ForgeCLI") && text.contains("deep-thinking on localhost:11434"), "{text}");
            assert!(text.split_whitespace().collect::<Vec<_>>().join(" ").contains(WELCOME_LINE), "{text}");
        }
        assert!(plain(&welcome(&DARK, &w, 100)).join("\n").contains("/ commands   @ files   ! shell   ? shortcuts"));
        assert!(plain(&welcome(&DARK, &w, 40)).iter().all(|l| !l.contains('▀')), "no logo when narrow");
    }

    #[test]
    fn the_first_run_card_says_how_to_set_up() {
        let text = plain(&first_run(&DARK, "0.1.0", "no API key for the Messages API", 100)).join("\n");
        for want in
            ["reforged", "FORGE_OPENAI_BASE_URL", "FORGE_API_KEY", "Forge found  no API key", "forge doctor --probe"]
        {
            assert!(text.contains(want), "{want}\n{text}");
        }
    }

    #[test]
    fn ansi_writes_the_styles() {
        let l = Line::from(vec![
            Span::styled("a", Style::default().fg(Color::Rgb(1, 2, 3)).bg(Color::Indexed(9))),
            Span::raw("b"),
        ]);
        assert_eq!(ansi(&l), "\x1b[38;2;1;2;3;48;5;9ma\x1b[0mb");
    }
}
