//! Configuration: layered settings, memory files and where resources live.

mod memory;
mod settings;

pub use memory::{load_memory, MemoryFile, MemoryKind};
pub use settings::{
    deep_merge, load_settings, parse_sources, write_setting, LoadedSettings, SettingSource, SettingsLayer,
    SettingsOptions,
};

use std::path::{Path, PathBuf};

/// `$FORGE_HOME` or `~/.forge`.
pub fn forge_home() -> PathBuf {
    std::env::var_os("FORGE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".forge"))
}

/// `$FORGE_CLAUDE_HOME` or `~/.claude` (read for compatibility).
pub fn claude_home() -> PathBuf {
    std::env::var_os("FORGE_CLAUDE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    User,
    Project,
}

/// Directories holding `kind` resources (`agents`, `commands`, `skills`,
/// `output-styles`), lowest precedence first: user before project, and
/// `.claude` before `.forge` within each level.
pub fn resource_dirs(kind: &str, project: &Path) -> Vec<(Scope, PathBuf)> {
    vec![
        (Scope::User, claude_home().join(kind)),
        (Scope::User, forge_home().join(kind)),
        (Scope::Project, project.join(".claude").join(kind)),
        (Scope::Project, project.join(".forge").join(kind)),
    ]
}
