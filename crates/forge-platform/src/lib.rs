//! OS differences in one place: which shell runs commands and how a command
//! is handed to it, how a process tree is stopped, and how paths are shown.
//! Everything that decides by OS is a pure function of an [`shell::Os`] value,
//! so the Windows rules are unit-tested on Linux CI.

pub mod path;
pub mod process;
pub mod shell;
