//! Plugins: a directory bundling commands, agents, skills, output styles,
//! hooks and MCP servers, loaded with `--plugin-dir` (or the `pluginDirs`
//! setting). Its manifest is `.forge-plugin/plugin.json`, the reference CLI's
//! `.claude-plugin/plugin.json`, or `plugin.json` (`name`, `description`,
//! `version`, and optionally `hooks` and `mcpServers`, inline or as a path);
//! without one, the directory's name is used.
//!
//! `${FORGE_PLUGIN_ROOT}` (or `${CLAUDE_PLUGIN_ROOT}`) in its hooks and MCP
//! servers is the plugin's folder, and `${FORGE_PLUGIN_DATA}` (or
//! `${CLAUDE_PLUGIN_DATA}`) a folder of its own that survives updates.
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
    /// The manifest as written (`Null` without one).
    pub manifest: Value,
}

impl Plugin {
    /// The plugin's hooks (the settings `hooks` shape): the manifest's `hooks`
    /// (inline, or a path in the plugin), else `hooks/hooks.json`.
    pub fn hooks(&self) -> Option<Value> {
        let v = match self.manifest.get("hooks") {
            Some(h) if h.is_object() => h.clone(),
            Some(Value::String(p)) => self.read_json(p)?,
            _ => self.read_json("hooks/hooks.json")?,
        };
        let mut v = v.get("hooks").cloned().unwrap_or(v);
        self.fill(&mut v);
        Some(v)
    }

    /// The plugin's MCP servers as `--mcp-config` JSON text: the manifest's
    /// `mcpServers` (inline, or a path in the plugin), else `.mcp.json`.
    pub fn mcp_config(&self) -> Option<String> {
        let v = match self.manifest.get("mcpServers") {
            Some(m) if m.is_object() => serde_json::json!({ "mcpServers": m }),
            Some(Value::String(p)) => self.read_json(p)?,
            _ => self.read_json(".mcp.json")?,
        };
        let mut v = if v.get("mcpServers").is_some() { v } else { serde_json::json!({ "mcpServers": v }) };
        self.fill(&mut v);
        Some(v.to_string())
    }

    /// Where the plugin keeps `kind` (`skills`, `commands`, `agents`,
    /// `output-styles`): its `<kind>/` folder, plus any paths its manifest
    /// gives for that key (a string or a list), as the reference CLI reads them.
    pub fn component_dirs(&self, kind: &str) -> Vec<PathBuf> {
        let mut out = vec![self.dir.join(kind)];
        let extra: Vec<&str> = match self.manifest.get(kind) {
            Some(Value::String(p)) => vec![p.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        for p in extra {
            let d = self.dir.join(p.trim_start_matches("./"));
            // Only inside the plugin.
            if d.starts_with(&self.dir) && !p.contains("..") && !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }

    /// The plugin's own folder for data that survives updates ([`data_dir`]).
    pub fn data_dir(&self) -> PathBuf {
        data_dir(&self.name)
    }

    fn read_json(&self, rel: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.dir.join(rel.trim_start_matches("./"))).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Put the plugin's folders in every string of `v`.
    fn fill(&self, v: &mut Value) {
        match v {
            Value::String(s) => *s = self.fill_text(s),
            Value::Array(a) => a.iter_mut().for_each(|x| self.fill(x)),
            Value::Object(o) => o.values_mut().for_each(|x| self.fill(x)),
            _ => {}
        }
    }

    /// `text` with `${FORGE_PLUGIN_ROOT}` / `${FORGE_PLUGIN_DATA}` (and the
    /// reference CLI's names for them) replaced by the plugin's folders.
    pub fn fill_text(&self, text: &str) -> String {
        if !text.contains("_PLUGIN_") {
            return text.to_string();
        }
        let root = self.dir.display().to_string();
        let mut out = text.replace("${FORGE_PLUGIN_ROOT}", &root).replace("${CLAUDE_PLUGIN_ROOT}", &root);
        if out.contains("_PLUGIN_DATA}") {
            let data = self.data_dir();
            let _ = std::fs::create_dir_all(&data);
            let data = data.display().to_string();
            out = out.replace("${FORGE_PLUGIN_DATA}", &data).replace("${CLAUDE_PLUGIN_DATA}", &data);
        }
        out
    }
}

/// A plugin's own folder for data that survives updates, by its name. Made
/// when something first refers to it.
pub fn data_dir(plugin: &str) -> PathBuf {
    forge_config::state_dir().join("plugin-data").join(plugin)
}

pub fn load_plugin(dir: &Path) -> Result<Plugin, String> {
    let dir = forge_platform::path::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let manifest = [
        dir.join(".forge-plugin").join("plugin.json"),
        dir.join(".claude-plugin").join("plugin.json"),
        dir.join("plugin.json"),
    ]
    .into_iter()
    .find(|p| p.is_file());
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
        manifest: meta,
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
        assert!(plugins[0].mcp_config().is_some() && plugins[1].mcp_config().is_none());
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn reference_cli_plugins_load_with_their_paths_filled_in() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("cc-plugin");
        std::fs::create_dir_all(p.join(".claude-plugin")).unwrap();
        std::fs::write(
            p.join(".claude-plugin/plugin.json"),
            r#"{"name": "formatter", "version": "2.0.0",
                "hooks": {"PostToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/fmt.sh\""}]}]},
                "mcpServers": {"db": {"command": "${CLAUDE_PLUGIN_ROOT}/db", "args": ["--data", "${FORGE_PLUGIN_DATA}"]}}}"#,
        )
        .unwrap();
        let mut w = vec![];
        let plugin = load_plugins(std::slice::from_ref(&p), &mut w).remove(0);
        assert_eq!((plugin.name.as_str(), plugin.version.as_deref()), ("formatter", Some("2.0.0")));
        let root = plugin.dir.display().to_string();
        let hooks = plugin.hooks().unwrap();
        assert_eq!(hooks["PostToolUse"][0]["hooks"][0]["command"], format!("\"{root}/fmt.sh\""));
        let mcp: Value = serde_json::from_str(&plugin.mcp_config().unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["db"]["command"], format!("{root}/db"));
        assert_eq!(mcp["mcpServers"]["db"]["args"][1], plugin.data_dir().display().to_string());
        assert!(w.is_empty(), "{w:?}");
    }
}
