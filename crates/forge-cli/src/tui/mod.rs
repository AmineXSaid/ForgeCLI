//! The interactive terminal UI (M8). The design and the build plan are in
//! docs/TUI.md.
//!
//! Phase 1 is in progress: the prompt editor (`editor`), text rendering and
//! wrapping (`text`) and the UI state machine (`app`) are built and tested.
//! The session task, the renderer and the terminal loop come next, and only
//! then is the UI wired into `main`; until then nothing outside the tests uses
//! these modules.
#![allow(dead_code)]

pub mod app;
pub mod editor;
pub mod text;
