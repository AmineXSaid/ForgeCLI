//! Plugins: a directory bundling commands, agents, skills, output styles,
//! hooks and MCP servers, loaded with `--plugin-dir` (or the `pluginDirs`
//! setting). Its manifest is `.forge-plugin/plugin.json` or `plugin.json`
//! (`name`, `description`, `version`); without one, the directory's name is
//! used.
//!
//! The layout:
//! - `commands/*.md` become `/<plugin>:<name>`;
//! - `agents/*.md`;
//! - `skills/<name>/SKILL.md` become `<plugin>:<name>`;
//! - `output-styles/*.md`;
//! - `hooks/hooks.json`, shaped like the settings `hooks` object;
//! - `.mcp.json`. These servers start without the `.mcp.json` approval
//!   step, since the user chose the plugin.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub dir: PathBuf,
}

impl Plugin {
    /// The plugin's `hooks/hooks.json` (the settings `hooks` shape), if any.
    pub fn hooks(&self) -> Option<Value> {
        let text = std::fs::read_to_string(self.dir.join("hooks").join("hooks.json")).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        Some(v.get("hooks").cloned().unwrap_or(v))
    }

    pub fn mcp_file(&self) -> Option<PathBuf> {
        Some(self.dir.join(".mcp.json")).filter(|p| p.is_file())
    }
}

pub fn load_plugin(dir: &Path) -> Result<Plugin, String> {
    let dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let manifest =
        [dir.join(".forge-plugin").join("plugin.json"), dir.join("plugin.json")].into_iter().find(|p| p.is_file());
    let meta: Value = match &manifest {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", p.display()))?,
        None => Value::Null,
    };
    let name = meta
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("plugin name {name:?}: use letters, digits, '-' and '_'"));
    }
    Ok(Plugin {
        name,
        description: meta.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
        version: meta.get("version").and_then(Value::as_str).map(str::to_string),
        dir,
    })
}

pub fn load_plugins(dirs: &[PathBuf], warnings: &mut Vec<String>) -> Vec<Plugin> {
    let mut out: Vec<Plugin> = vec![];
    for d in dirs {
        match load_plugin(d) {
            Ok(p) if out.iter().any(|o| o.name == p.name) => {
                warnings.push(format!("plugin {} loaded twice; keeping the first ({})", p.name, d.display()))
            }
            Ok(p) => out.push(p),
            Err(e) => warnings.push(format!("plugin: {e}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_and_components() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("my-plugin");
        std::fs::create_dir_all(p.join(".forge-plugin")).unwrap();
        std::fs::create_dir_all(p.join("hooks")).unwrap();
        std::fs::write(p.join(".forge-plugin/plugin.json"), r#"{"name": "tools-kit", "version": "1.0.0"}"#).unwrap();
        std::fs::write(p.join("hooks/hooks.json"), r#"{"hooks": {"Stop": []}}"#).unwrap();
        std::fs::write(p.join(".mcp.json"), "{}").unwrap();
        let bare = d.path().join("bare_one");
        std::fs::create_dir_all(&bare).unwrap();
        let mut w = vec![];
        let plugins = load_plugins(&[p.clone(), bare, d.path().join("missing")], &mut w);
        assert_eq!(plugins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["tools-kit", "bare_one"]);
        assert_eq!(plugins[0].version.as_deref(), Some("1.0.0"));
        assert_eq!(plugins[0].hooks().unwrap(), serde_json::json!({"Stop": []}));
        assert!(plugins[0].mcp_file().is_some() && plugins[1].mcp_file().is_none());
        assert_eq!(w.len(), 1, "{w:?}");
    }
}
