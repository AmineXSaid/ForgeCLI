//! Exit statuses (public interface; see docs/CLI.md).

use std::fmt;

/// The command did what was asked.
pub const OK: i32 = 0;
/// The run failed: API error, tool or agent failure, or a prompt blocked by a hook.
pub const FAILED: i32 = 1;
/// Invalid usage: unknown flag, bad value, missing argument, or a terminal was needed.
pub const USAGE: i32 = 2;
/// Configuration or credentials are missing or invalid.
pub const CONFIG: i32 = 3;
/// A limit stopped the run: --max-turns or --max-budget-usd.
pub const LIMIT: i32 = 4;
/// Interrupted (Ctrl-C), following the shell convention 128 + SIGINT.
pub const INTERRUPTED: i32 = 130;

/// An error that knows its exit status and the next useful action.
#[derive(Debug)]
pub struct Fail {
    pub code: i32,
    pub message: String,
    pub hint: Option<String>,
    /// No endpoint is set up: the terminal UI shows its first-run card.
    pub first_run: bool,
}

impl Fail {
    pub fn usage(message: impl Into<String>) -> Self {
        Fail { code: USAGE, message: message.into(), hint: None, first_run: false }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Fail { code: CONFIG, message: message.into(), hint: None, first_run: false }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(h) = &self.hint {
            write!(f, "\n  hint: {h}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Fail {}

impl From<forge_core::CoreError> for Fail {
    fn from(e: forge_core::CoreError) -> Self {
        use forge_core::CoreError::*;
        match e {
            Auth(m) => Fail {
                first_run: true,
                ..Fail::config(m).with_hint("Run `forge doctor` to see which endpoint and key Forge found.")
            },
            Config(m) => Fail::config(m),
            Engine(e) => Fail::config(e.to_string()),
            Session(forge_session::SessionError::NotFound(id)) => Fail::config(format!("no session {id}"))
                .with_hint("List sessions with `forge --resume` in that project."),
            Session(forge_session::SessionError::InvalidId(id)) => {
                Fail::usage(format!("invalid session id {id:?}: session ids are UUIDs"))
            }
            Session(e) => Fail::config(e.to_string()),
            Api(e) => Fail {
                code: if e.is_auth_failure() { CONFIG } else { FAILED },
                message: e.to_string(),
                hint: e.hint(),
                first_run: false,
            },
        }
    }
}

/// Exit status for a finished run. An interrupt requested by a host over
/// the control channel is a normal outcome (0); Ctrl-C is handled by the
/// caller, which exits with [`INTERRUPTED`].
pub fn for_result(r: &forge_engine::TurnResult) -> i32 {
    use forge_types::sdk::ResultSubtype::*;
    if r.prompt_blocked.is_some() {
        return FAILED;
    }
    if r.auth_failed {
        return CONFIG;
    }
    match r.subtype {
        Success => OK,
        ErrorMaxTurns | ErrorMaxBudgetUsd => LIMIT,
        ErrorDuringExecution | ErrorMaxStructuredOutputRetries => FAILED,
    }
}
