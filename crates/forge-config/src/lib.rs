//! Configuration: layered settings, memory files (FORGE.md) and where resources live.

mod memory;
mod settings;

pub use memory::{load_memory, MemoryFile, MemoryKind};
pub use redact::{is_secret_key, redact};

mod redact;
pub use settings::{
    deep_merge, load_settings, parse_sources, write_setting, LoadedSettings, SettingSource, SettingsLayer,
    SettingsOptions,
};

use std::path::{Path, PathBuf};

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// User configuration: settings.json, FORGE.md, agents, commands, skills.
///
/// `$FORGE_HOME` if set; else `$XDG_CONFIG_HOME/forge`; else the platform's
/// config directory (`~/.config/forge` on Linux,
/// `~/Library/Application Support/forge` on macOS, `%APPDATA%\forge` on Windows).
pub fn config_dir() -> PathBuf {
    env_dir("FORGE_HOME")
        .or_else(|| env_dir("XDG_CONFIG_HOME").map(|d| d.join("forge")))
        .or_else(|| dirs::config_dir().map(|d| d.join("forge")))
        .unwrap_or_else(|| home().join(".forge"))
}

/// Sessions, file history and other state: `$FORGE_HOME/state`, else
/// `$XDG_STATE_HOME/forge`, else the platform's local data directory.
pub fn state_dir() -> PathBuf {
    env_dir("FORGE_HOME")
        .map(|d| d.join("state"))
        .or_else(|| env_dir("XDG_STATE_HOME").map(|d| d.join("forge")))
        .or_else(|| dirs::state_dir().or_else(dirs::data_local_dir).map(|d| d.join("forge")))
        .unwrap_or_else(|| home().join(".forge").join("state"))
}

/// Disposable caches: `$FORGE_HOME/cache`, else `$XDG_CACHE_HOME/forge`, else the platform cache directory.
pub fn cache_dir() -> PathBuf {
    env_dir("FORGE_HOME")
        .map(|d| d.join("cache"))
        .or_else(|| env_dir("XDG_CACHE_HOME").map(|d| d.join("forge")))
        .or_else(|| dirs::cache_dir().map(|d| d.join("forge")))
        .unwrap_or_else(|| home().join(".forge").join("cache"))
}

/// The user configuration directory (kept for callers that predate the XDG split).
pub fn forge_home() -> PathBuf {
    config_dir()
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
/// `output-styles`), lowest precedence first: user, then the project's
/// vendor-neutral `.agents/<kind>` (shared with other agent CLIs), then its
/// `.forge/<kind>`.
pub fn resource_dirs(kind: &str, project: &Path) -> Vec<(Scope, PathBuf)> {
    vec![
        (Scope::User, forge_home().join(kind)),
        (Scope::Project, project.join(".agents").join(kind)),
        (Scope::Project, project.join(".forge").join(kind)),
    ]
}
