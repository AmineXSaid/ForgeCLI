//! Forge's mark in the terminal: the F-and-cube logo, drawn as a pixel sprite
//! in half blocks (each cell is two pixels stacked, so the pixels come out
//! square), and the two screens it heads: the welcome banner and the
//! first-run card. The F with a cube for its lower arm, flat, in paper (or
//! ink), warm grey and Forge's copper.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::palette;
use super::text::{width, Theme};

/// The logo: a smith's hammer, one character per pixel, lit from above;
/// `.` is empty. The head is steel (`a` its lit top edge, `b` its body, `c`
/// its shadow) with a rounded peen on the left and a striking face on the
/// right that glows copper (`k`), hot from the forge; a copper collar holds
/// it on a wooden handle (`w` lit, `d` shaded) wrapped in a leather grip
/// (`e`) with a copper band.
pub const MARK: [&str; 16] = [
    "................",
    "...aaaaaaaaaa...",
    "..abbbbbbbbbbbk.",
    ".abbbbbbbbbbbbkk",
    ".bbbbbbbbbbbbbkk",
    ".cbbbbbbbbbbbbkk",
    "..ccccccccccccc.",
    "......kkkk......",
    ".......wd.......",
    ".......wd.......",
    ".......wd.......",
    ".......ee.......",
    ".......kk.......",
    ".......ee.......",
    ".......ee.......",
    ".......dd.......",
];

/// The 12-pixel cut, for narrow terminals.
pub const MARK_SMALL: [&str; 12] = [
    "............",
    "..aaaaaaaa..",
    ".abbbbbbbbk.",
    ".bbbbbbbbbkk",
    ".cbbbbbbbbkk",
    "..ccccccccc.",
    ".....kk.....",
    ".....wd.....",
    ".....wd.....",
    ".....ee.....",
    ".....kk.....",
    ".....dd.....",
];

/// The logo's inks for a light or dark background: hex, and the ANSI colour
/// for 16-colour terminals. Steel in three tones, Forge's copper for the hot
/// face, the collar and the band, and warm wood and leather.
fn mark_ink(c: char, light: bool) -> Option<(u32, Color)> {
    Some(match (c, light) {
        ('a', false) => (0xf2eee9, Color::White),
        ('b', false) => (0xbcb5ad, Color::Gray),
        ('c', false) => (0x7a736b, Color::DarkGray),
        ('k', false) => (0xe2895b, Color::LightYellow),
        ('w', false) => (0xc48d5c, Color::Yellow),
        ('d', false) => (0x8c5c36, Color::Yellow),
        ('e', false) => (0x7a4a2c, Color::Red),
        ('a', true) => (0x9a948d, Color::Gray),
        ('b', true) => (0x67615b, Color::DarkGray),
        ('c', true) => (0x3d3833, Color::Black),
        ('k', true) => (0xb4532a, Color::Yellow),
        ('w', true) => (0xa8743f, Color::Yellow),
        ('d', true) => (0x7a4f28, Color::Yellow),
        ('e', true) => (0x5a3520, Color::Red),
        _ => return None,
    })
}

/// A sprite in half blocks. Without colour the shape is drawn in the
/// terminal's own colour.
fn sprite(rows: &[&str], ink: fn(char, bool) -> Option<(u32, Color)>, t: &Theme) -> Vec<Line<'static>> {
    let color_of = |c: Option<char>| -> Option<Color> {
        let (hex, ansi) = ink(c?, t.light)?;
        if !t.color {
            return Some(Color::Reset);
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

/// Forge at text size, for one-line places: the hammer three cells wide, in
/// half blocks so its pixels are square: a steel head with its copper face
/// over the handle. Without colour, the same shape in the terminal's colour.
pub fn mark_inline(t: &Theme) -> Vec<Span<'static>> {
    let ink = |c| mark_ink(c, t.light).map(|(hex, ansi)| palette::ink(hex, t.depth, ansi));
    match (t.color, ink('b'), ink('k'), ink('w')) {
        (true, Some(steel), Some(copper), Some(wood)) => vec![
            Span::styled("▀", Style::default().fg(steel)),
            Span::styled("▀", Style::default().fg(steel).bg(wood)),
            Span::styled("▀", Style::default().fg(copper)),
        ],
        _ => vec![Span::raw("▀█▀")],
    }
}

/// A line centred in `cols` columns.
fn centred(l: Line<'static>, cols: usize) -> Line<'static> {
    let w: usize = l.spans.iter().map(|s| width(&s.content)).sum();
    let mut spans = vec![Span::raw(" ".repeat(cols.saturating_sub(w) / 2))];
    spans.extend(l.spans);
    trim_end(Line::from(spans))
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

/// `s` in at most `max` columns, its end kept: `…/live/proj`.
fn keep_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut tail: Vec<char> = vec![];
    let mut w = 1;
    for c in s.chars().rev() {
        let cw = width(&c.to_string());
        if w + cw > max {
            break;
        }
        w += cw;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

/// What the welcome banner says.
pub struct Welcome<'a> {
    pub version: &'a str,
    pub model: &'a str,
    /// Where requests go: a host, or the kind of endpoint.
    pub endpoint: &'a str,
    /// The working directory, as people write it (`~/proj`).
    pub cwd: &'a str,
    /// The newest other conversations here: (what it was about, how long ago).
    pub recent: &'a [(String, String)],
}

/// The keys worth knowing on the first screen, key first.
pub const KEYS: [(&str, &str); 4] = [("/", "commands"), ("@", "files"), ("!", "shell"), ("?", "shortcuts")];

/// The line under the banner, as Forge's welcome page says it.
pub const WELCOME_LINE: &str = "What to do first? Ask about this codebase or we can start writing code.";

fn keys_line(t: &Theme, gap: usize) -> Line<'static> {
    keys_of(t, &KEYS, gap)
}

fn keys_of(t: &Theme, keys: &[(&str, &str)], gap: usize) -> Line<'static> {
    let mut spans = vec![];
    for (i, (k, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled(k.to_string(), t.accent().add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {what}"), t.dim()));
    }
    Line::from(spans)
}

/// The front screen printed once at the top of a session, centred: the
/// hammer, ForgeCLI and its version, the model and where it runs, the
/// directory; then the recent conversations here, the keys to know and the
/// welcome line. `cols` is the terminal's width; narrow terminals get the
/// small hammer, or none, and lines keep their ends rather than wrap.
pub fn welcome(t: &Theme, w: &Welcome, cols: usize) -> Vec<Line<'static>> {
    let room = cols.saturating_sub(4).max(8);
    let mut out = vec![Line::default()];
    let art = if cols >= 41 { Some(mark(t, cols < 60)) } else { None };
    let line = |out: &mut Vec<Line<'static>>, l: Line<'static>| {
        out.push(if art.is_some() { centred(l, cols) } else { indent(l) })
    };
    if let Some(a) = &art {
        for l in a {
            out.push(centred(l.clone(), cols));
        }
        out.push(Line::default());
    }
    line(
        &mut out,
        Line::from(vec![
            Span::styled("ForgeCLI", t.brand_strong().add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", w.version), t.dim()),
        ]),
    );
    let endpoint = keep_end(w.endpoint, room.saturating_sub(width(w.model) + 4).max(8));
    line(
        &mut out,
        Line::from(vec![
            Span::styled(w.model.to_string(), t.bold()),
            Span::styled(" on ", t.dim()),
            Span::styled(endpoint, t.dim()),
        ]),
    );
    line(&mut out, Line::from(Span::styled(keep_end(w.cwd, room), t.subtle())));
    // The recent conversations: a block as wide as its longest row, centred as one.
    if !w.recent.is_empty() {
        out.push(Line::default());
        let bw = room.min(60);
        let block_x = if art.is_some() { cols.saturating_sub(bw) / 2 } else { 2 };
        let pad = " ".repeat(block_x);
        let head_r = "/resume";
        out.push(Line::from(vec![
            Span::raw(pad.clone()),
            Span::styled("Recent", t.dim().add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(bw.saturating_sub(6 + head_r.len()))),
            Span::styled(head_r, t.accent()),
        ]));
        for (what, ago) in w.recent.iter().take(3) {
            let what = super::render::fit(what, bw.saturating_sub(width(ago) + 2));
            out.push(Line::from(vec![
                Span::raw(pad.clone()),
                Span::styled(what.clone(), Style::default()),
                Span::raw(" ".repeat(bw.saturating_sub(width(&what) + width(ago)))),
                Span::styled(ago.clone(), t.subtle()),
            ]));
        }
    }
    out.push(Line::default());
    if cols >= 52 {
        line(&mut out, keys_line(t, 3));
    } else {
        line(&mut out, keys_of(t, &KEYS[..2], 2));
        line(&mut out, keys_of(t, &KEYS[2..], 2));
    }
    line(&mut out, Line::from(Span::styled(WELCOME_LINE, t.subtle())));
    out.push(Line::default());
    super::text::wrap(out, cols as u16)
}

fn indent(l: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    spans.extend(l.spans);
    Line::from(spans)
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
    use crate::tui::palette::Depth;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect()
    }

    const DARK: Theme = Theme { color: true, light: false, accent: None, depth: Depth::True };

    #[test]
    fn sprites_are_square_pixels_in_half_blocks() {
        for rows in [&MARK[..], &MARK_SMALL[..]] {
            assert!(rows.iter().all(|r| r.chars().count() == rows.len()), "square");
            assert!(
                rows.iter().flat_map(|r| r.chars()).all(|c| c == '.' || mark_ink(c, false).is_some()),
                "every pixel has an ink"
            );
        }
        let m = mark(&DARK, false);
        assert_eq!(m.len(), 8);
        assert!(m.iter().all(|l| l.spans.iter().map(|s| width(&s.content)).sum::<usize>() <= 16));
        assert_eq!(mark(&DARK, true).len(), 6);
        // The head's lit top edge, then its body, in steel; the striking face in copper.
        let cells = |l: &Line| l.spans.iter().flat_map(|s| s.content.chars().map(move |_| s.style)).collect::<Vec<_>>();
        let top = cells(&m[0]);
        assert_eq!(top[5].fg, Some(Color::Rgb(0xf2, 0xee, 0xe9)), "{top:?}");
        let copper = Some(Color::Rgb(0xe2, 0x89, 0x5b));
        assert!(m.iter().flat_map(|l| &l.spans).any(|s| s.style.fg == copper || s.style.bg == copper));
        // On a light background the steel darkens and the copper deepens.
        let light = mark(&Theme { light: true, ..DARK }, false);
        let deep = Some(Color::Rgb(0xb4, 0x53, 0x2a));
        assert!(light.iter().flat_map(|l| &l.spans).any(|s| s.style.fg == deep || s.style.bg == deep));
    }

    #[test]
    fn without_colour_the_mark_keeps_its_shape() {
        let mono = Theme { color: false, ..DARK };
        let m = plain(&mark(&mono, false));
        assert!(m[1].contains("██████"), "the head: {m:?}");
        assert_eq!(m[5].trim(), "██", "the handle: {m:?}");
        assert!(mark(&mono, false)
            .iter()
            .all(|l| l.spans.iter().all(|s| s.style.fg.is_none() && s.style.bg.is_none())));
    }

    #[test]
    fn the_inline_mark_is_a_small_hammer() {
        let m = mark_inline(&DARK);
        assert_eq!(plain(&[Line::from(m.clone())]), ["▀▀▀"]);
        assert_eq!(m[2].style.fg, Some(Color::Rgb(0xe2, 0x89, 0x5b)), "the copper face");
        assert!(m[1].style.bg.is_some() && m[0].style.bg.is_none(), "the handle under the middle");
        assert_eq!(plain(&[Line::from(mark_inline(&Theme { color: false, ..DARK }))]), ["▀█▀"]);
    }

    #[test]
    fn the_front_screen_is_centred_and_fits() {
        let recent = vec![("checkout rounding".to_string(), "2h ago".to_string())];
        let w = Welcome {
            version: "0.1.0",
            model: "deep-thinking",
            endpoint: "localhost:11434",
            cwd: "~/proj",
            recent: &recent,
        };
        for cols in [120, 80, 60, 50, 40] {
            let lines = welcome(&DARK, &w, cols);
            for l in &lines {
                let lw: usize = l.spans.iter().map(|s| width(&s.content)).sum();
                assert!(lw <= cols, "{cols}: {lw} {:?}", plain(std::slice::from_ref(l)));
            }
            // Words, so a line wrapped on a narrow terminal still counts.
            let text = plain(&lines).join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
            for want in [
                "ForgeCLI 0.1.0",
                "deep-thinking on localhost:11434",
                "~/proj",
                "Recent",
                "/resume",
                "checkout rounding",
                "2h ago",
                WELCOME_LINE,
            ] {
                assert!(text.contains(want), "{cols}: {want}\n{text}");
            }
        }
        // Wide: the whole hammer and the title, both centred.
        let wide = plain(&welcome(&DARK, &w, 100));
        let title = wide.iter().find(|l| l.contains("ForgeCLI")).unwrap();
        let lead = title.len() - title.trim_start().len();
        assert!((lead as i32 - (100 - 15) / 2).abs() <= 1, "centred: {title:?}");
        assert_eq!(wide.iter().filter(|l| l.contains('▀') || l.contains('▄') || l.contains('█')).count(), 8);
        assert!(wide.iter().any(|l| l.trim() == "/ commands   @ files   ! shell   ? shortcuts"));
        // A side pane: the small hammer, the keys two to a line.
        let side = plain(&welcome(&DARK, &w, 48));
        assert_eq!(
            side.iter().filter(|l| l.contains('▀') || l.contains('▄') || l.contains('█')).count(),
            6,
            "{side:?}"
        );
        assert!(side.iter().any(|l| l.trim() == "/ commands  @ files"));
        // Narrow: no art, lines from the left.
        assert!(plain(&welcome(&DARK, &w, 40)).iter().all(|l| !l.contains('▀')), "no logo when narrow");
        // A long folder keeps its end.
        let deep = Welcome { cwd: "/tmp/claude-0/-home-user-forge-gpui/3b3af639/scratchpad/live/proj", ..w };
        assert!(plain(&welcome(&DARK, &deep, 54)).iter().any(|l| l.ends_with("/live/proj") && l.contains('…')));
        // No other conversations: no Recent block.
        let none = Welcome { recent: &[], ..w };
        assert!(!plain(&welcome(&DARK, &none, 80)).join("\n").contains("Recent"));
        assert_eq!(keep_end("abcdef", 4), "…def");
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
