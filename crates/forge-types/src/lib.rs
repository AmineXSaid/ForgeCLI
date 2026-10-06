//! Wire types shared by every ForgeCLI crate.
//!
//! * [`api`] mirrors the Anthropic Messages API: content blocks, messages,
//!   usage and the server-sent stream events.
//! * [`sdk`] is the newline-delimited JSON protocol spoken on stdout/stdin in
//!   `--output-format stream-json` / `--input-format stream-json` mode,
//!   including the bidirectional control channel.

pub mod api;
pub mod sdk;

pub use api::*;

/// A fresh random identifier (UUID v4, hyphenated).
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
