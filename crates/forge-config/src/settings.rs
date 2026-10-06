use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::forge_home;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
    Flag,
    Managed,
}

impl SettingSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingSource::User => "userSettings",
            SettingSource::Project => "projectSettings",
            SettingSource::Local => "localSettings",
            SettingSource::Flag => "flagSettings",
            SettingSource::Managed => "policySettings",
        }
    }
}

/// Parse `--setting-sources user,project,local`.
pub fn parse_sources(raw: &str) -> Result<Vec<SettingSource>, String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s {
            "user" => Ok(SettingSource::User),
            "project" => Ok(SettingSource::Project),
            "local" => Ok(SettingSource::Local),
            other => Err(format!("unknown setting source {other:?} (expected user, project or local)")),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct SettingsLayer {
    pub source: SettingSource,
    pub path: Option<PathBuf>,
    pub value: Value,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedSettings {
    pub merged: Value,
    pub layers: Vec<SettingsLayer>,
    /// Files that exist but failed to parse (ignored, reported).
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SettingsOptions {
    pub project_dir: PathBuf,
    /// Which file-based layers to read (`--setting-sources`); flag and managed always load.
    pub sources: Vec<SettingSource>,
    /// `--settings`: a path or an inline JSON object.
    pub flag: Option<String>,
    pub managed_path: PathBuf,
}

impl SettingsOptions {
    pub fn new(project_dir: &Path) -> Self {
        SettingsOptions {
            project_dir: project_dir.to_path_buf(),
            sources: vec![SettingSource::User, SettingSource::Project, SettingSource::Local],
            flag: None,
            managed_path: PathBuf::from("/etc/forge/managed-settings.json"),
        }
    }
}

/// Merge `b` into `a`: objects recursively, arrays concatenated without
/// duplicates, everything else replaced.
pub fn deep_merge(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in y {
                match x.get_mut(k) {
                    Some(existing) => deep_merge(existing, v),
                    None => {
                        x.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            for v in y {
                if !x.contains(v) {
                    x.push(v.clone());
                }
            }
        }
        (a, b) => *a = b.clone(),
    }
}

fn read_json(path: &Path, errors: &mut Vec<String>) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(v @ Value::Object(_)) => Some(v),
        Ok(_) => {
            errors.push(format!("{}: settings must be a JSON object", path.display()));
            None
        }
        Err(e) => {
            errors.push(format!("{}: {e}", path.display()));
            None
        }
    }
}

/// Load and merge every layer (later wins).
pub fn load_settings(opts: &SettingsOptions) -> LoadedSettings {
    let mut out = LoadedSettings { merged: Value::Object(Map::new()), ..Default::default() };
    let p = &opts.project_dir;
    let mut files: Vec<(SettingSource, PathBuf)> = vec![];
    for src in [SettingSource::User, SettingSource::Project, SettingSource::Local] {
        if !opts.sources.contains(&src) {
            continue;
        }
        match src {
            SettingSource::User => files.push((src, forge_home().join("settings.json"))),
            SettingSource::Project => files.push((src, p.join(".forge/settings.json"))),
            SettingSource::Local => files.push((src, p.join(".forge/settings.local.json"))),
            _ => {}
        }
    }
    for (src, path) in files {
        if let Some(v) = read_json(&path, &mut out.errors) {
            out.layers.push(SettingsLayer { source: src, path: Some(path), value: v });
        }
    }
    if let Some(flag) = &opts.flag {
        let t = flag.trim();
        let parsed = if t.starts_with('{') {
            serde_json::from_str::<Value>(t).map_err(|e| format!("--settings: {e}")).map(|v| (v, None))
        } else {
            let path = PathBuf::from(t);
            match read_json(&path, &mut out.errors) {
                Some(v) => Ok((v, Some(path))),
                None if !path.exists() => Err(format!("--settings: {} does not exist", path.display())),
                None => Err(String::new()),
            }
        };
        match parsed {
            Ok((v, path)) => out.layers.push(SettingsLayer { source: SettingSource::Flag, path, value: v }),
            Err(e) if !e.is_empty() => out.errors.push(e),
            Err(_) => {}
        }
    }
    if let Some(v) = read_json(&opts.managed_path, &mut out.errors) {
        out.layers.push(SettingsLayer {
            source: SettingSource::Managed,
            path: Some(opts.managed_path.clone()),
            value: v,
        });
    }
    for layer in &out.layers {
        deep_merge(&mut out.merged, &layer.value);
    }
    out
}

impl LoadedSettings {
    pub fn get(&self, pointer: &str) -> Option<&Value> {
        self.merged.pointer(pointer)
    }

    pub fn str(&self, pointer: &str) -> Option<&str> {
        self.get(pointer).and_then(Value::as_str)
    }

    pub fn strings(&self, pointer: &str) -> Vec<String> {
        self.get(pointer)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub fn bool(&self, pointer: &str) -> Option<bool> {
        self.get(pointer).and_then(Value::as_bool)
    }

    /// `env` block as key/value pairs.
    pub fn env(&self) -> Vec<(String, String)> {
        self.get("/env")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The managed layer, if any (its permission rules cannot be overridden).
    pub fn managed(&self) -> Option<&Value> {
        self.layers.iter().find(|l| l.source == SettingSource::Managed).map(|l| &l.value)
    }
}

/// Write `key = value` into a settings file (creating it), keeping other keys.
pub fn write_setting(path: &Path, pointer: &[&str], value: Value) -> std::io::Result<()> {
    let mut root: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut cur = &mut root;
    for (i, key) in pointer.iter().enumerate() {
        if i + 1 == pointer.len() {
            cur[*key] = value.clone();
        } else {
            if !cur.get(*key).map(Value::is_object).unwrap_or(false) {
                cur[*key] = Value::Object(Map::new());
            }
            cur = cur.get_mut(*key).unwrap();
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&root)? + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layers_merge_in_order() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("proj");
        std::fs::create_dir_all(p.join(".forge")).unwrap();
        std::fs::write(p.join(".forge/settings.json"), r#"{"model":"a","permissions":{"allow":["Read"]}}"#).unwrap();
        // The local layer is malformed: it is skipped and reported.
        std::fs::write(p.join(".forge/settings.local.json"), "{not json").unwrap();
        let mut opts = SettingsOptions::new(&p);
        opts.sources = vec![SettingSource::Project, SettingSource::Local];
        opts.flag = Some(r#"{"model":"b","permissions":{"allow":["Bash(ls)"],"deny":["Bash(rm *)"]}}"#.into());
        opts.managed_path = d.path().join("managed.json");
        std::fs::write(&opts.managed_path, r#"{"model":"managed"}"#).unwrap();
        let s = load_settings(&opts);
        assert_eq!(s.str("/model"), Some("managed"));
        assert_eq!(s.strings("/permissions/allow"), vec!["Read", "Bash(ls)"]);
        assert_eq!(s.strings("/permissions/deny"), vec!["Bash(rm *)"]);
        assert_eq!(s.errors.len(), 1, "{:?}", s.errors);
    }

    #[test]
    fn sources_filter_layers() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".forge")).unwrap();
        std::fs::write(d.path().join(".forge/settings.json"), r#"{"model":"x"}"#).unwrap();
        let mut opts = SettingsOptions::new(d.path());
        opts.sources = vec![SettingSource::Local];
        opts.managed_path = d.path().join("none.json");
        assert_eq!(load_settings(&opts).str("/model"), None);
        assert!(parse_sources("user,bogus").is_err());
    }

    #[test]
    fn write_setting_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.json");
        std::fs::write(&f, r#"{"model":"x"}"#).unwrap();
        write_setting(&f, &["permissions", "defaultMode"], json!("plan")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v, json!({"model":"x","permissions":{"defaultMode":"plan"}}));
    }
}
