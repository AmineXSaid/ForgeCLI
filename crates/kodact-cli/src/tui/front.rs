//! The full-screen front page, one per style (`look.rs`): what the session
//! has, laid out before the first message. Pixel art (the wordmark in four
//! looks, the anvil) is drawn in half blocks, two square pixels per cell.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::brand::{centred, GLYPHS, TAGLINE};
use super::look::Look;
use super::palette;
use super::render::fit;
use super::text::{width, Theme};

/// One pixel: its colour (hex) and the ANSI colour for 16-colour terminals.
type Px = Option<(u32, Color)>;

/// How the wordmark is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Letters {
    /// Paper letters.
    Flat,
    /// Paper letters over an ember shadow, one pixel down and right.
    Shadow,
    /// Metal out of the fire: gold at the top, ember at the foot.
    Heat,
    /// The letters' edges only, in copper.
    Outline,
}

/// What the front page shows about the session.
#[derive(Debug, Clone, Default)]
pub struct FrontInfo {
    pub version: String,
    pub model: String,
    pub endpoint: String,
    /// The folder, as people write it (`~/proj`).
    pub folder: String,
    pub branch: Option<String>,
    /// MCP servers: (name, status).
    pub mcp: Vec<(String, String)>,
    pub tools: usize,
    /// The newest other conversations here: (what, how long ago).
    pub recent: Vec<(String, String)>,
}

/// A front page for `full_view`: rows pinned to the top, rows above and below
/// the input, where the input sits and how wide it is.
#[derive(Debug, Clone, Default)]
pub struct FrontPage {
    /// Rows at the top of the screen, whatever its height.
    pub header: Vec<Line<'static>>,
    pub above: Vec<Line<'static>>,
    pub below: Vec<Line<'static>>,
    /// The input's first column and width.
    pub box_left: usize,
    pub box_w: usize,
    /// The mode, model and endpoint on a line under the input.
    pub under: bool,
    /// The input sits at the bottom of the screen, not with the page.
    pub dock: bool,
    /// The page starts at the top (under the header) instead of centred down the screen.
    pub top: bool,
}

fn ink(hex: u32, ansi: Color, t: &Theme) -> Color {
    if !t.color {
        return Color::Reset;
    }
    palette::ink(hex, t.depth, ansi)
}

/// A grid of pixels in half blocks: one line per two rows.
fn half_blocks(grid: &[Vec<Px>], t: &Theme) -> Vec<Line<'static>> {
    let colour = |p: Px| p.map(|(hex, ansi)| ink(hex, ansi, t));
    let fg = |c: Color| if c == Color::Reset { Style::default() } else { Style::default().fg(c) };
    let w = grid.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![];
    for pair in grid.chunks(2) {
        let mut spans: Vec<Span<'static>> = vec![];
        for x in 0..w {
            let u = pair[0].get(x).copied().flatten().and_then(|p| colour(Some(p)));
            let l = pair.get(1).and_then(|r| r.get(x).copied().flatten()).and_then(|p| colour(Some(p)));
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
        while spans.last().is_some_and(|s| s.content.trim().is_empty() && s.style.bg.is_none()) {
            spans.pop();
        }
        out.push(Line::from(spans));
    }
    out
}

/// The wordmark's pixels in one of its looks: 47 by 10, 48 by 11 with a shadow.
fn wordmark_grid(look: Letters, light: bool) -> Vec<Vec<Px>> {
    let (paper, copper, lit, edge, shade, shadow) = if light {
        (
            (0x2b2825, Color::Black),
            (0xb4532a, Color::Yellow),
            (0x5a544e, Color::DarkGray),
            (0x433e39, Color::Black),
            (0xa59e97, Color::Gray),
            (0xd9b49c, Color::Yellow),
        )
    } else {
        (
            (0xefe9e4, Color::White),
            (0xe2895b, Color::LightYellow),
            (0xc4bbb3, Color::Gray),
            (0x8f877f, Color::DarkGray),
            (0x6f6963, Color::DarkGray),
            (0x7a3a1f, Color::Red),
        )
    };
    // Gold at the top cooling to ember at the foot.
    const HEAT: [(u32, Color); 10] = [
        (0xffe3a1, Color::LightYellow),
        (0xffd27a, Color::LightYellow),
        (0xffc160, Color::Yellow),
        (0xf9ab52, Color::Yellow),
        (0xf2954a, Color::Yellow),
        (0xea8045, Color::LightRed),
        (0xe26e40, Color::LightRed),
        (0xd65d3a, Color::Red),
        (0xc84d34, Color::Red),
        (0xb8402f, Color::Red),
    ];
    let (w, h) = if look == Letters::Shadow { (48, 11) } else { (47, 10) };
    let mut letter = vec![vec![None::<char>; w]; h];
    for (gi, rows) in GLYPHS.iter().enumerate() {
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                if c != '.' {
                    letter[y][gi * 10 + x] = Some(c);
                }
            }
        }
    }
    let at = |x: isize, y: isize| -> Option<char> {
        (x >= 0 && y >= 0).then(|| letter.get(y as usize).and_then(|r| r.get(x as usize)).copied().flatten()).flatten()
    };
    let mut grid = vec![vec![None; w]; h];
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let (xi, yi) = (x as isize, y as isize);
            *cell = match at(xi, yi) {
                Some('T') => Some(copper),
                Some('L') => Some(lit),
                Some('E') => Some(edge),
                Some('S') => Some(shade),
                Some(_) => match look {
                    Letters::Heat => Some(HEAT[y.min(9)]),
                    Letters::Outline => {
                        let inside =
                            [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|(dx, dy)| at(xi + dx, yi + dy).is_some());
                        (!inside).then_some(copper)
                    }
                    _ => Some(paper),
                },
                None if look == Letters::Shadow && at(xi - 1, yi - 1).is_some() => Some(shadow),
                None => None,
            };
        }
    }
    grid
}

/// The wordmark in `look`: 5 rows (6 with a shadow).
pub fn wordmark(t: &Theme, look: Letters) -> Vec<Line<'static>> {
    half_blocks(&wordmark_grid(look, t.light), t)
}

/// The wordmark's width in columns.
pub fn wordmark_w(look: Letters) -> usize {
    if look == Letters::Shadow {
        48
    } else {
        47
    }
}

/// KodAct's anvil: a hot ingot on its face, two sparks, a horn to the left and
/// two small eyes. Original pixel art, 24 by 16 (24 columns by 8 rows).
const ANVIL: [&str; 16] = [
    "..............y.........",
    ".........y..............",
    "..........hhhhhh....y...",
    ".........hooooooh.......",
    ".........rrrrrrrr.......",
    "...AAAAAAAAAAAAAAAAAA...",
    "AAAABBBBBBBBBBBBBBBBBC..",
    ".ABBBBBBBBBBBBBBBBBBBC..",
    "...BBBBKBBBBBBKBBBBBC...",
    ".....BBBBBBBBBBBBBBC....",
    "........BBBBBBBC........",
    "........BBBBBBBC........",
    ".......BBBBBBBBBC.......",
    ".....BBBBBBBBBBBBBC.....",
    "....BBBBBBBBBBBBBBBC....",
    "....CCCCCCCCCCCCCCCC....",
];

/// The anvil's width in columns.
pub const ANVIL_W: usize = 24;

pub fn anvil(t: &Theme) -> Vec<Line<'static>> {
    let px = |c: char| -> Px {
        Some(match (c, t.light) {
            ('y', _) => (0xffd27a, Color::LightYellow),
            ('h', _) => (0xffe3a1, Color::LightYellow),
            ('o', _) => (0xf9ab52, Color::Yellow),
            ('r', _) => (0xe2593a, Color::Red),
            ('A', false) => (0xe4ddd6, Color::White),
            ('B', false) => (0x9a928a, Color::Gray),
            ('C', false) => (0x5f5952, Color::DarkGray),
            ('K', false) => (0x1b1916, Color::Black),
            ('A', true) => (0x8f8a84, Color::Gray),
            ('B', true) => (0x5f5a54, Color::DarkGray),
            ('C', true) => (0x3a3631, Color::Black),
            ('K', true) => (0xf7f3ee, Color::White),
            _ => return None,
        })
    };
    let grid: Vec<Vec<Px>> = ANVIL.iter().map(|r| r.chars().map(px).collect()).collect();
    half_blocks(&grid, t)
}

// ---- layout helpers ----

fn line_w(l: &Line) -> usize {
    l.spans.iter().map(|s| width(&s.content)).sum()
}

/// Art `w` columns wide, centred in `cols` as one block, so its rows stay aligned.
fn centre_block(lines: Vec<Line<'static>>, w: usize, cols: usize) -> Vec<Line<'static>> {
    let x = cols.saturating_sub(w) / 2;
    lines.into_iter().map(|l| at(x, l)).collect()
}

/// `l` starting at column `x`.
fn at(x: usize, l: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(x))];
    spans.extend(l.spans);
    Line::from(spans)
}

/// Two blocks side by side: `left` at column `x`, `right` at column `rx`, top-aligned.
fn beside(x: usize, left: Vec<Line<'static>>, rx: usize, right: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let n = left.len().max(right.len());
    (0..n)
        .map(|i| {
            let l = left.get(i).cloned().unwrap_or_default();
            let used = x + line_w(&l);
            let mut spans = vec![Span::raw(" ".repeat(x))];
            spans.extend(l.spans);
            if let Some(r) = right.get(i) {
                spans.push(Span::raw(" ".repeat(rx.saturating_sub(used))));
                spans.extend(r.spans.clone());
            }
            Line::from(spans)
        })
        .collect()
}

/// `left` at column `x` and `right` ending at column `end`.
fn spread(x: usize, left: Vec<Span<'static>>, end: usize, right: Vec<Span<'static>>) -> Line<'static> {
    let lw: usize = left.iter().map(|s| width(&s.content)).sum();
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    let mut spans = vec![Span::raw(" ".repeat(x))];
    spans.extend(left);
    spans.push(Span::raw(" ".repeat(end.saturating_sub(x + lw + rw).max(1))));
    spans.extend(right);
    Line::from(spans)
}

/// `s` in at most `max` columns, its end kept: `…/acme/shop`.
fn keep_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut tail: Vec<char> = vec![];
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = width(&c.to_string());
        if used + cw > max {
            break;
        }
        used += cw;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

fn mcp_text(info: &FrontInfo) -> Option<String> {
    let on: Vec<&str> = info.mcp.iter().filter(|(_, s)| s == "connected").map(|(n, _)| n.as_str()).collect();
    (!on.is_empty()).then(|| on.join(", "))
}

fn resume_spans(t: &Theme, what: &str, ago: &str, room: usize) -> Vec<Span<'static>> {
    vec![
        Span::styled("/resume", t.accent()),
        Span::raw("  "),
        Span::styled(fit(what, room.saturating_sub(width(ago) + 14).max(8)), t.dim()),
        Span::styled("  ·  ", t.hairline()),
        Span::styled(ago.to_string(), t.subtle()),
    ]
}

fn wrap_at(x: usize, text: &str, w: usize, st: Style) -> Vec<Line<'static>> {
    super::text::wrap(vec![Line::from(Span::styled(text.to_string(), st))], w as u16)
        .into_iter()
        .map(|l| at(x, l))
        .collect()
}

/// `l` cut to `w` columns.
fn clip(l: Line<'static>, w: usize) -> Line<'static> {
    let mut used = 0;
    let mut spans = vec![];
    for s in l.spans {
        if used >= w {
            break;
        }
        let room = w - used;
        if width(&s.content) <= room {
            used += width(&s.content);
            spans.push(s);
        } else {
            let mut text = String::new();
            for c in s.content.chars() {
                let cw = width(&c.to_string());
                if used + cw > w {
                    break;
                }
                used += cw;
                text.push(c);
            }
            spans.push(Span::styled(text, s.style));
        }
    }
    Line::from(spans)
}

/// A rounded box `w` wide at column `x`, its rows padded inside.
fn boxed(x: usize, w: usize, rows: Vec<Line<'static>>, border: Style) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4);
    let mut out = vec![at(x, Line::from(Span::styled(format!("╭{}╮", "─".repeat(w.saturating_sub(2))), border)))];
    for r in rows {
        let r = clip(r, inner);
        let used = line_w(&r);
        let mut spans = vec![Span::raw(" ".repeat(x)), Span::styled("│ ", border)];
        spans.extend(r.spans);
        spans.push(Span::raw(" ".repeat(inner - used)));
        spans.push(Span::styled(" │", border));
        out.push(Line::from(spans));
    }
    out.push(at(x, Line::from(Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border))));
    out
}

// ---- the pages ----

/// The front page for `look`, `cols` wide. A terminal too narrow for a style's
/// layout gets the bare one.
pub fn front_page(t: &Theme, look: Look, info: &FrontInfo, cols: usize) -> FrontPage {
    let need = match look {
        Look::Forgeframe | Look::Anvil => 84,
        Look::Ledger | Look::Status => 80,
        Look::Molten | Look::Bare => 0,
    };
    if cols < need {
        return bare(t, info, cols);
    }
    match look {
        Look::Forgeframe => forgeframe(t, info, cols),
        Look::Molten => molten(t, info, cols),
        Look::Anvil => anvil_card(t, info, cols),
        Look::Ledger => ledger(t, info, cols),
        Look::Status => status_board(t, info, cols),
        Look::Bare => bare(t, info, cols),
    }
}

/// The name in letters too wide for the terminal: `F O R G E` in one row.
fn small_name(t: &Theme) -> Span<'static> {
    Span::styled("F O R G E", t.brand_strong().add_modifier(Modifier::BOLD))
}

/// 1 · A framed welcome: the wordmark with an ember shadow beside the anvil,
/// then the version, what KodAct does, what is connected, and the folder.
fn forgeframe(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let m = 4;
    let fw = cols - 2 * m;
    let corner = t.subtle();
    let mut above = vec![spread(m, vec![Span::styled("┌─", corner)], m + fw, vec![Span::styled("─┐", corner)])];
    let mut left = vec![Line::from(Span::styled("Welcome to", t.dim())), Line::default()];
    left.extend(wordmark(t, Letters::Shadow));
    let tag = "a coding agent, reforged";
    left.push(Line::from(vec![
        Span::raw(" ".repeat(wordmark_w(Letters::Shadow).saturating_sub(width(tag)))),
        Span::styled(tag, t.subtle()),
    ]));
    let right = if fw >= wordmark_w(Letters::Shadow) + ANVIL_W + 16 { anvil(t) } else { vec![] };
    above.extend(beside(m + 4, left, m + fw - 4 - ANVIL_W, right));
    above.push(spread(m, vec![Span::styled("└─", corner)], m + fw, vec![Span::styled("─┘", corner)]));
    above.push(Line::default());
    above.push(at(
        m,
        Line::from(vec![
            Span::styled(format!("Version {}  ·  ", info.version), t.subtle()),
            Span::styled(info.model.clone(), t.dim()),
            Span::styled(format!(" on {}", info.endpoint), t.subtle()),
        ]),
    ));
    above.push(Line::default());
    above.extend(wrap_at(
        m,
        "KodAct reads, edits and runs code in this folder, on the model you choose. Describe a task to begin; it asks before it changes anything outside your files.",
        fw.min(100),
        Style::default(),
    ));
    above.push(Line::default());
    let bullet = |c: Style, text: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled("●  ", c)];
        spans.extend(text);
        at(m, Line::from(spans))
    };
    match mcp_text(info) {
        Some(names) => above.push(bullet(t.success(), vec![Span::raw("Connected to "), Span::styled(names, t.dim())])),
        None => above.push(bullet(t.success(), vec![Span::raw(format!("{} tools ready", info.tools))])),
    }
    if let Some((what, ago)) = info.recent.first() {
        above.push(bullet(
            t.accent(),
            vec![
                Span::raw("Last time: "),
                Span::styled(fit(what, fw.saturating_sub(40)), t.dim()),
                Span::styled(format!("  ·  {ago}  ·  "), t.subtle()),
                Span::styled("/resume", t.accent()),
            ],
        ));
    }
    above.push(Line::default());
    let mut folder = vec![Span::styled(keep_end(&info.folder, fw.saturating_sub(40)), t.bold())];
    if let Some(b) = &info.branch {
        folder.push(Span::styled(format!("  [⎇ {b}]"), t.dim()));
    }
    above.push(at(m, Line::from(folder)));
    FrontPage { above, box_left: m, box_w: fw, top: true, ..Default::default() }
}

/// 2 · Centred, the name as hot metal with sparks over it and a glowing seam under it.
fn molten(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let mut above = vec![];
    let ww = wordmark_w(Letters::Heat);
    if cols >= ww + 4 {
        // Sparks rising off the letters, at fixed places.
        let spark = |hex: u32| Style::default().fg(ink(hex, Color::Yellow, t));
        let rows: [&[(usize, &str, u32)]; 2] = [
            &[(9, "·", 0xffd27a), (21, "˙", 0xffe3a1), (33, "·", 0xffc160)],
            &[(4, "˙", 0xf9ab52), (15, "·", 0xffd27a), (27, "·", 0xf2954a), (40, "˙", 0xffc160)],
        ];
        for r in rows {
            let mut spans = vec![];
            let mut x = 0;
            for (at_x, ch, hex) in r {
                spans.push(Span::raw(" ".repeat(at_x - x)));
                spans.push(Span::styled(ch.to_string(), if t.color { spark(*hex) } else { t.subtle() }));
                x = at_x + 1;
            }
            above.push(at(cols.saturating_sub(ww) / 2, Line::from(spans)));
        }
        above.extend(centre_block(wordmark(t, Letters::Heat), ww, cols));
        // The seam: brightest in the middle.
        let n = 40usize;
        let seam: Vec<Span<'static>> = (0..n)
            .map(|i| {
                let d = (i as f32 - n as f32 / 2.0).abs() / (n as f32 / 2.0);
                let hex = if d < 0.2 {
                    0xffd27a
                } else if d < 0.5 {
                    0xf2954a
                } else if d < 0.8 {
                    0xc84d34
                } else {
                    0x5e2a1c
                };
                Span::styled("━", if t.color { Style::default().fg(ink(hex, Color::Yellow, t)) } else { t.subtle() })
            })
            .collect();
        above.push(centred(Line::from(seam), cols));
    } else {
        above.push(centred(Line::from(small_name(t)), cols));
    }
    above.push(Line::default());
    let tagline = Line::from(Span::styled(TAGLINE, t.dim()));
    above.extend(
        super::text::wrap(vec![tagline], cols.saturating_sub(4).max(8) as u16).into_iter().map(|l| centred(l, cols)),
    );
    above.push(Line::default());
    above.push(Line::default());
    let bw = cols.saturating_sub(8).min(72);
    let mut below = vec![];
    if let Some((what, ago)) = info.recent.first() {
        below.push(Line::default());
        below.push(centred(Line::from(resume_spans(t, what, ago, cols)), cols));
    }
    FrontPage { above, below, box_left: (cols - bw) / 2, box_w: bw, under: true, ..Default::default() }
}

/// 3 · A card: the anvil beside the name and the session's readouts; the input under it.
fn anvil_card(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let cw = cols.saturating_sub(8).min(86);
    let x = (cols - cw) / 2;
    let border = t.hairline();
    let inner = cw - 4;
    let mut rows = vec![];
    rows.push(spread(
        0,
        vec![Span::styled("kodact", t.accent()), Span::styled(format!("  v{}", info.version), t.subtle())],
        inner,
        vec![Span::styled("● ", t.success()), Span::styled("ready", t.dim())],
    ));
    rows.push(Line::from(Span::styled("─".repeat(inner), border)));
    rows.push(Line::default());
    let mut right = wordmark(t, Letters::Flat);
    right.push(Line::default());
    let tag = if inner >= ANVIL_W + 5 + width(TAGLINE) { TAGLINE } else { "a coding agent, reforged" };
    right.push(Line::from(Span::styled(tag, t.dim())));
    rows.extend(beside(1, anvil(t), ANVIL_W + 5, right));
    rows.push(Line::default());
    let kv = |k: &str, v: String, d: String| {
        Line::from(vec![Span::styled(format!(" {k:<8}"), t.subtle()), Span::raw(v), Span::styled(d, t.subtle())])
    };
    rows.push(kv("model", info.model.clone(), format!("  on {}", info.endpoint)));
    let tools = match mcp_text(info) {
        Some(n) => format!("  ·  {n}"),
        None => String::new(),
    };
    rows.push(kv("tools", format!("{} built in", info.tools), tools));
    let branch = info.branch.as_ref().map(|b| format!("  ·  {b}")).unwrap_or_default();
    rows.push(kv("folder", keep_end(&info.folder, inner.saturating_sub(30)), branch));
    let mut below = vec![];
    if let Some((what, ago)) = info.recent.first() {
        below.push(Line::default());
        below.push(centred(Line::from(resume_spans(t, what, ago, cols)), cols));
    }
    FrontPage { above: boxed(x, cw, rows, border), below, box_left: x, box_w: cw, ..Default::default() }
}

/// 4 · Aligned left: the name in copper outline, past work numbered beside this folder,
/// the input docked at the bottom.
fn ledger(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let m = 8;
    let end = cols - m;
    let mut header = vec![];
    let side = vec![
        Line::default(),
        Line::from(Span::styled(format!("v{}", info.version), t.subtle())),
        Line::from(Span::styled(info.model.clone(), t.dim())),
        Line::from(Span::styled(info.endpoint.clone(), t.subtle())),
    ];
    let sw = side.iter().map(line_w).max().unwrap_or(0);
    header.extend(beside(m, wordmark(t, Letters::Outline), end - sw, side));
    header.push(Line::default());
    header.push(at(m, Line::from(Span::styled("─".repeat(end - m), t.hairline()))));
    header.push(Line::default());
    let half = m + (end - m) / 2 + 4;
    let mut left = vec![Line::from(Span::styled("Pick up where you left off", t.subtle())), Line::default()];
    if info.recent.is_empty() {
        left.push(Line::from(Span::styled("Nothing yet in this folder.", t.dim())));
    }
    for (i, (what, ago)) in info.recent.iter().take(3).enumerate() {
        left.push(Line::from(vec![
            Span::styled(format!("{:<4}", i + 1), t.accent()),
            Span::raw(fit(what, half.saturating_sub(m + 20))),
            Span::styled(format!("  {ago}"), t.subtle()),
        ]));
    }
    let mut right = vec![
        Line::from(Span::styled("This folder", t.subtle())),
        Line::default(),
        Line::from(keep_end(&info.folder, end.saturating_sub(half))),
    ];
    if let Some(b) = &info.branch {
        right.push(Line::from(Span::styled(format!("⎇ {b}"), t.dim())));
    }
    if let Some(n) = mcp_text(info) {
        right.push(Line::from(vec![Span::styled("● ", t.success()), Span::styled(n, t.dim())]));
    }
    header.extend(beside(m, left, half, right));
    // The page sits centred down the screen, above the docked input.
    FrontPage { above: header, box_left: 2, box_w: cols - 4, dock: true, ..Default::default() }
}

/// 5 · The name over three boxed readouts of the session, the input as wide as them.
fn status_board(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let total = cols.saturating_sub(8).min(96);
    let x = (cols - total) / 2;
    let mut above = centre_block(wordmark(t, Letters::Shadow), wordmark_w(Letters::Shadow), cols);
    above.push(Line::default());
    above.push(Line::default());
    let tw = (total - 4) / 3;
    let tile = |k: &str, v: String, d: String| -> Vec<Line<'static>> {
        boxed(
            0,
            tw,
            vec![
                Line::from(Span::styled(k.to_string(), t.accent())),
                Line::from(fit(&v, tw - 4)),
                Line::from(Span::styled(fit(&d, tw - 4), t.subtle())),
            ],
            t.hairline(),
        )
    };
    let tiles = [
        tile("Model", info.model.clone(), info.endpoint.clone()),
        tile(
            "Tools",
            format!("{} built in", info.tools),
            mcp_text(info).map(|n| format!("{n} over MCP")).unwrap_or_else(|| "no MCP servers".into()),
        ),
        tile("Folder", keep_end(&info.folder, tw - 4), info.branch.clone().unwrap_or_default()),
    ];
    for r in 0..tiles[0].len() {
        let mut spans = vec![Span::raw(" ".repeat(x))];
        for (i, tl) in tiles.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.extend(tl[r].spans.clone());
        }
        above.push(Line::from(spans));
    }
    above.push(Line::default());
    let mut below = vec![];
    if let Some((what, ago)) = info.recent.first() {
        below.push(Line::default());
        below.push(centred(Line::from(resume_spans(t, what, ago, cols)), cols));
    }
    FrontPage { above, below, box_left: x, box_w: total, ..Default::default() }
}

/// 6 · A small name in the corner, one question and the input in the middle, recent work under it.
fn bare(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    // The model and endpoint on the right, when there is room for them beside the name.
    let where_ = format!("{} · {}", info.model, info.endpoint);
    let right = if cols >= 2 + 9 + 2 + width(&info.version) + 4 + width(&where_) + 2 {
        vec![Span::styled(where_, t.subtle())]
    } else {
        vec![]
    };
    // No room beside the name: the model goes under the input instead.
    let under = right.is_empty();
    let header = vec![
        Line::default(),
        spread(
            2,
            vec![small_name(t), Span::styled(format!("  {}", info.version), t.subtle())],
            cols.saturating_sub(2),
            right,
        ),
    ];
    let bw = cols.saturating_sub(4).min(72);
    let x = (cols - bw) / 2;
    let above = vec![centred(Line::from(Span::styled("What shall we kodact today?", t.bold())), cols), Line::default()];
    let mut below = vec![];
    if !info.recent.is_empty() {
        below.push(Line::default());
        for (what, ago) in info.recent.iter().take(3) {
            below.push(spread(
                x + 2,
                vec![Span::styled(fit(what, bw.saturating_sub(20)), t.dim())],
                x + bw - 2,
                vec![Span::styled(ago.clone(), t.subtle())],
            ));
        }
    }
    FrontPage { header, above, below, box_left: x, box_w: bw, under, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::palette::Depth;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(crate::tui::text::plain).collect()
    }

    const DARK: Theme = Theme { color: true, light: false, accent: None, depth: Depth::True };

    fn info() -> FrontInfo {
        FrontInfo {
            version: "0.1.0".into(),
            model: "stand-in".into(),
            endpoint: "127.0.0.1:11434".into(),
            folder: "~/acme/shop".into(),
            branch: Some("fix/cart-rounding".into()),
            mcp: vec![("jira".into(), "connected".into()), ("figma".into(), "failed".into())],
            tools: 18,
            recent: vec![("checkout rounding".into(), "2h ago".into())],
        }
    }

    #[test]
    fn the_wordmark_has_four_looks_and_the_anvil_is_whole() {
        assert_eq!(wordmark(&DARK, Letters::Flat).len(), 5);
        assert_eq!(wordmark(&DARK, Letters::Shadow).len(), 6, "the shadow adds a pixel row");
        let heat = wordmark(&DARK, Letters::Heat);
        let fg = |l: &Line| {
            l.spans.iter().find(|s| s.content.contains('█') || s.content.contains('▀')).and_then(|s| s.style.fg)
        };
        assert_ne!(fg(&heat[0]), fg(&heat[4]), "hot at the top, cooler at the foot");
        let outline = plain(&wordmark(&DARK, Letters::Outline));
        assert!(outline.iter().any(|l| l.contains(' ')), "hollow letters");
        let a = anvil(&DARK);
        assert_eq!(a.len(), 8);
        assert!(a.iter().all(|l| line_w(l) <= ANVIL_W));
    }

    #[test]
    fn every_style_fits_and_says_what_the_session_has() {
        for look in Look::ALL {
            for cols in [140, 100, 84, 60, 40] {
                let p = front_page(&DARK, look, &info(), cols);
                let rows: Vec<&Line> = p.header.iter().chain(&p.above).chain(&p.below).collect();
                for l in &rows {
                    assert!(line_w(l) <= cols, "{look:?} {cols}: {:?}", plain(std::slice::from_ref(*l)));
                }
                assert!(p.box_left + p.box_w <= cols, "{look:?} {cols}: the input fits");
                let text = rows.iter().map(|l| crate::tui::text::plain(l)).collect::<Vec<_>>().join("\n");
                assert!(text.contains("stand-in") || p.under, "{look:?} {cols}: the model shows\n{text}");
            }
        }
        let frame = plain(&front_page(&DARK, Look::Forgeframe, &info(), 120).above).join("\n");
        for want in [
            "Welcome to",
            "Connected to jira",
            "Last time: checkout rounding",
            "/resume",
            "~/acme/shop  [⎇ fix/cart-rounding]",
        ] {
            assert!(frame.contains(want), "{want}\n{frame}");
        }
        assert!(!frame.contains("figma"), "only connected servers");
        let ledger = front_page(&DARK, Look::Ledger, &info(), 120);
        assert!(ledger.dock && plain(&ledger.above).join("\n").contains("1   checkout rounding"));
        let board = plain(&front_page(&DARK, Look::Status, &info(), 120).above).join("\n");
        assert!(board.contains("Model") && board.contains("Tools") && board.contains("Folder"));
        // Too narrow for a style's layout: the bare page.
        let narrow = front_page(&DARK, Look::Forgeframe, &info(), 60);
        assert!(plain(&narrow.header).join("").contains("F O R G E"));
    }
}
