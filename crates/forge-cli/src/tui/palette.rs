//! Forge's colours in the terminal: the ember palette (warm ink, paper and
//! copper), one value per role for a dark and for a light background.
//! A role is drawn in 24-bit colour where the terminal has it, else as the
//! nearest of xterm's 256 colours, else as one of the 16 ANSI colours.

use ratatui::style::Color;

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    /// 24-bit colour.
    #[default]
    True,
    /// xterm's 256 colours.
    Ansi256,
    /// The 16 ANSI colours, in the person's own palette.
    Ansi16,
}

impl Depth {
    /// From `FORGE_COLOR_DEPTH` (`truecolor`, `256`, `16`), else what the terminal says of itself.
    pub fn detect() -> Depth {
        let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
        Depth::from_env(&var("FORGE_COLOR_DEPTH"), &var("COLORTERM"), &var("TERM"), &var("TERM_PROGRAM"), {
            std::env::var_os("WT_SESSION").is_some()
        })
    }

    fn from_env(forced: &str, colorterm: &str, term: &str, program: &str, windows_terminal: bool) -> Depth {
        match forced {
            "truecolor" | "24bit" | "true" => return Depth::True,
            "256" => return Depth::Ansi256,
            "16" | "8" => return Depth::Ansi16,
            _ => {}
        }
        if matches!(colorterm, "truecolor" | "24bit")
            || term.contains("direct")
            || windows_terminal
            || matches!(program, "iterm.app" | "wezterm" | "vscode" | "ghostty" | "hyper" | "tabby")
        {
            Depth::True
        } else if term.contains("256") {
            Depth::Ansi256
        } else if cfg!(windows) && term.is_empty() {
            // Windows 10's console has drawn 24-bit colour since 1703.
            Depth::True
        } else {
            Depth::Ansi16
        }
    }
}

/// Whether the terminal's background is light, from `COLORFGBG` ("15;0" is
/// light text on black). `None` when the terminal doesn't say.
pub fn light_background() -> Option<bool> {
    let v = std::env::var("COLORFGBG").ok()?;
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(matches!(bg, 7 | 15))
}

/// One role: its dark- and light-background colours, and the ANSI colour it
/// falls back to in 16-colour terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Role {
    pub dark: u32,
    pub light: u32,
    pub ansi_dark: Color,
    pub ansi_light: Color,
}

const fn role(dark: u32, light: u32, ansi_dark: Color, ansi_light: Color) -> Role {
    Role { dark, light, ansi_dark, ansi_light }
}

impl Role {
    pub fn color(&self, depth: Depth, light: bool) -> Color {
        match depth {
            Depth::True => rgb(if light { self.light } else { self.dark }),
            Depth::Ansi256 => {
                let (r, g, b) = split(if light { self.light } else { self.dark });
                Color::Indexed(nearest_256(r, g, b))
            }
            Depth::Ansi16 => {
                if light {
                    self.ansi_light
                } else {
                    self.ansi_dark
                }
            }
        }
    }
}

// The roles: Forge's ember. Warm ink and paper for text and surfaces, copper
// for the brand and for what needs the person, and colour elsewhere only
// where it means something (done, failed, added, removed, code).

/// The brand: the prompt mark, the spinner, selections, the keys, headings.
pub const BRAND: Role = role(0xe2895b, 0xb4532a, Color::LightYellow, Color::Yellow);
/// The brand a step brighter, for emphasis: the wordmark, the shimmer's peak.
pub const BRAND_STRONG: Role = role(0xf3b694, 0x9a4420, Color::LightYellow, Color::Yellow);
/// Secondary text.
pub const MUTED: Role = role(0xb1a9a1, 0x5b534b, Color::Gray, Color::DarkGray);
/// Placeholders, hints, details.
pub const SUBTLE: Role = role(0x7b736b, 0x8d847a, Color::DarkGray, Color::DarkGray);
/// Rules, gutters and box edges.
pub const HAIRLINE: Role = role(0x3a3530, 0xcfc4b6, Color::DarkGray, Color::Gray);
/// Links and paths.
pub const INFO: Role = role(0x9db8cf, 0x3a6285, Color::LightBlue, Color::Blue);
/// Done.
pub const SUCCESS: Role = role(0x86b58f, 0x3d7a4d, Color::LightGreen, Color::Green);
/// Needs a look.
pub const WARNING: Role = role(0xe0b060, 0x93601a, Color::LightYellow, Color::Yellow);
/// Failed.
pub const DANGER: Role = role(0xe27a68, 0xb4473a, Color::LightRed, Color::Red);
/// A diff's added line.
pub const ADDED: Role = role(0x86b58f, 0x3d7a4d, Color::LightGreen, Color::Green);
/// A diff's removed line.
pub const REMOVED: Role = role(0xe27a68, 0xb4473a, Color::LightRed, Color::Red);
/// Inline code.
pub const CODE: Role = role(0xe9c9a8, 0x7c4a22, Color::LightMagenta, Color::Magenta);
/// The person's prompt: a raised surface.
pub const PROMPT_BG: Role = role(0x1e1b18, 0xece5dc, Color::DarkGray, Color::Gray);
/// The Molten style's band behind the person's words: a warm tint.
pub const WARM_BG: Role = role(0x2a1d15, 0xf3e3d6, Color::DarkGray, Color::Gray);
/// Text on [`PROMPT_BG`].
pub const PROMPT_FG: Role = role(0xede8e3, 0x1f1b17, Color::White, Color::Black);
/// The working shimmer's resting and brightest tones: warm grey up to copper.
pub const SHIMMER_BASE: Role = role(0x8a8178, 0x8d847a, Color::Gray, Color::DarkGray);
pub const SHIMMER_PEAK: Role = role(0xf3b694, 0xb4532a, Color::White, Color::Black);

// Code blocks: one quiet text colour and a few token colours, all warm.

/// Plain code.
pub const CODE_TEXT: Role = role(0xcfc7bf, 0x3a342e, Color::Gray, Color::DarkGray);
/// Keywords: `fn`, `def`, `const`, `if`.
pub const CODE_KEYWORD: Role = role(0xd9a07c, 0xa4481f, Color::LightYellow, Color::Yellow);
/// String literals.
pub const CODE_STRING: Role = role(0xaec596, 0x4a7a3c, Color::LightGreen, Color::Green);
/// Numbers and constants.
pub const CODE_NUMBER: Role = role(0xe3bd85, 0x93601a, Color::LightMagenta, Color::Magenta);
/// Comments.
pub const CODE_COMMENT: Role = role(0x7b736b, 0x8d847a, Color::DarkGray, Color::DarkGray);
/// Type names.
pub const CODE_TYPE: Role = role(0xe9c9a8, 0x7c4a22, Color::LightCyan, Color::Cyan);

pub const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn split(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A colour of the logo: 24-bit, the nearest of 256, or `ansi`.
pub fn ink(hex: u32, depth: Depth, ansi: Color) -> Color {
    match depth {
        Depth::True => rgb(hex),
        Depth::Ansi256 => {
            let (r, g, b) = split(hex);
            Color::Indexed(nearest_256(r, g, b))
        }
        Depth::Ansi16 => ansi,
    }
}

/// The xterm colour (16-255) closest to `r g b`: the 6x6x6 cube or the grey ramp.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let step = |v: u8| -> usize {
        let mut best = 0;
        for (i, s) in STEPS.iter().enumerate() {
            if (v as i32 - *s as i32).abs() < (v as i32 - STEPS[best] as i32).abs() {
                best = i;
            }
        }
        best
    };
    let dist = |a: (u8, u8, u8)| -> i32 {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        // Weighted for the eye: green counts most, blue least.
        3 * d(a.0, r) + 4 * d(a.1, g) + 2 * d(a.2, b)
    };
    let (ri, gi, bi) = (step(r), step(g), step(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let cube_index = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let grey_i = (((avg as i32 - 8).max(0) + 5) / 10).min(23) as u8;
    let grey = 8 + 10 * grey_i;
    if dist((grey, grey, grey)) < dist(cube) {
        232 + grey_i
    } else {
        cube_index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_comes_from_what_the_terminal_says() {
        assert_eq!(Depth::from_env("", "truecolor", "xterm-256color", "", false), Depth::True);
        assert_eq!(Depth::from_env("", "", "xterm-256color", "", false), Depth::Ansi256);
        assert_eq!(Depth::from_env("", "", "screen", "", true), Depth::True, "Windows Terminal");
        assert_eq!(Depth::from_env("", "", "xterm", "iterm.app", false), Depth::True);
        if !cfg!(windows) {
            assert_eq!(Depth::from_env("", "", "xterm", "", false), Depth::Ansi16);
        }
        assert_eq!(Depth::from_env("256", "truecolor", "", "", false), Depth::Ansi256, "the override wins");
    }

    #[test]
    fn the_nearest_256_colour_is_the_right_one() {
        assert_eq!(nearest_256(0xff, 0xff, 0xff), 231);
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
        // Forge's brand on a dark background: xterm 141 (#af87ff) or a close neighbour.
        let brand = nearest_256(0x94, 0x75, 0xdb);
        assert!([98, 104, 140, 141].contains(&brand), "{brand}");
    }

    #[test]
    fn roles_differ_for_light_and_dark_backgrounds() {
        assert_eq!(BRAND.color(Depth::True, false), Color::Rgb(0xe2, 0x89, 0x5b));
        assert_eq!(BRAND.color(Depth::True, true), Color::Rgb(0xb4, 0x53, 0x2a));
        assert_eq!(BRAND.color(Depth::Ansi16, false), Color::LightYellow);
        assert!(matches!(BRAND.color(Depth::Ansi256, false), Color::Indexed(_)));
    }
}
