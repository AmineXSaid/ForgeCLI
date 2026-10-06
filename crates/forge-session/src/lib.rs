//! Session persistence (contract C5) and file checkpoints (contract C4).

mod history;
mod store;
mod transcript;

pub use history::{FileHistory, RewindPlan};
pub use store::{forge_home, project_key, SessionStore, SessionSummary};
pub use transcript::{Entry, LoadedSession, Transcript};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session {0} not found")]
    NotFound(String),
    #[error("invalid session id {0:?}")]
    InvalidId(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("corrupt session file: {0}")]
    Corrupt(String),
}

/// Session ids are UUIDs; anything else is refused before it touches a path.
pub fn validate_session_id(id: &str) -> Result<(), SessionError> {
    let ok = id.len() == 36
        && id
            .chars()
            .enumerate()
            .all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() });
    if ok {
        Ok(())
    } else {
        Err(SessionError::InvalidId(id.to_string()))
    }
}

pub(crate) fn sha_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
