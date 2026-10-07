//! Forge's own marks for transcripts and the terminal UI, kept in one place so
//! every front end draws the same ones.

/// The first line of an answer.
pub const ANSWER: &str = "•";
/// A tool call: `› Name(argument)`.
pub const TOOL: &str = "›";
/// A tool's result, or a command's output, under it.
pub const RESULT: &str = "↳";
/// The spinner while a turn runs: braille dots.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Status-line marks for the permission modes.
pub const MODE_ACCEPT_EDITS: &str = "»";
pub const MODE_PLAN: &str = "‖";
pub const MODE_BYPASS: &str = "!";
