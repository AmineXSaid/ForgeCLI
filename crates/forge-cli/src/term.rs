//! Terminal capabilities and color, decided once at startup.

use std::io::IsTerminal;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub struct Term {
    pub stdin_tty: bool,
    /// Use ANSI colors (decided from --color, NO_COLOR, FORCE_COLOR and the terminals).
    pub color: bool,
}

static TERM: OnceLock<Term> = OnceLock::new();

fn env_set(k: &str) -> bool {
    std::env::var_os(k).map(|v| !v.is_empty()).unwrap_or(false)
}

/// Decide color: --color wins; then NO_COLOR (off); then FORCE_COLOR / CLICOLOR_FORCE (on);
/// then auto: stderr is a terminal and TERM is not "dumb".
pub fn init(choice: ColorChoice) -> Term {
    let stderr_tty = std::io::stderr().is_terminal();
    let color = match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if env_set("NO_COLOR") {
                false
            } else if env_set("FORCE_COLOR") || env_set("CLICOLOR_FORCE") {
                true
            } else {
                stderr_tty && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
            }
        }
    };
    let stdout_tty = std::io::stdout().is_terminal();
    // The interactive display writes to stdout: color it only when stdout is a terminal too.
    let t = Term {
        stdin_tty: std::io::stdin().is_terminal(),
        color: color && (stdout_tty || choice == ColorChoice::Always),
    };
    let _ = TERM.set(t);
    t
}

pub fn get() -> Term {
    *TERM.get().unwrap_or(&Term { stdin_tty: false, color: false })
}

/// Wrap `s` in an ANSI style when color is on.
pub fn paint(code: &str, s: &str) -> String {
    if get().color {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn dim(s: &str) -> String {
    paint("2", s)
}

pub fn red(s: &str) -> String {
    paint("31", s)
}

pub fn yellow(s: &str) -> String {
    paint("33", s)
}

pub fn bold(s: &str) -> String {
    paint("1", s)
}
