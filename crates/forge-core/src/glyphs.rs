//! Forge's own marks for transcripts and the terminal UI, kept in one place so
//! every front end draws the same ones.

/// The first line of an answer: a dot on Forge's timeline.
pub const ANSWER: &str = "●";
/// A tool call: `● Name(argument)`; in the terminal UI the dot takes the
/// call's state (running, done, failed).
pub const TOOL: &str = "●";
/// A tool's result, or a command's output, under it.
pub const RESULT: &str = "└";
/// The prompt: the person's turn, in the input and the transcript.
pub const PROMPT: &str = "❯";
/// A running tool's dot: a voxel turning, as Forge's working dot does.
pub const RUNNING: [&str; 4] = ["▖", "▘", "▝", "▗"];
/// The spinner while a turn runs: braille dots.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Status-line marks for the permission modes.
pub const MODE_ACCEPT_EDITS: &str = "»";
pub const MODE_PLAN: &str = "‖";
pub const MODE_BYPASS: &str = "!";
