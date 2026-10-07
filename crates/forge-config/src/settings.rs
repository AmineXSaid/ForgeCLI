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
    /// Precedence: a higher rank wins.
    pub fn rank(&self) -> u8 {
        match self {
            SettingSource::User => 0,
            SettingSource::Project => 1,
            SettingSource::Local => 2,
            SettingSource::Flag => 3,
            SettingSource::Managed => 4,
        }
    }

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
    /// The highest-precedence layer above `source` that sets `pointer`: the one
    /// a value written at `source` would lose to.
    pub fn overridden_by(&self, source: SettingSource, pointer: &str) -> Option<&SettingsLayer> {
        self.layers
            .iter()
            .filter(|l| l.source.rank() > source.rank() && l.value.pointer(pointer).is_some_and(|v| !v.is_null()))
            .max_by_key(|l| l.source.rank())
    }

    /// Mirror a write to `source`'s file in memory (`None` removes the key), so
    /// later reads in this process see it.
    pub fn apply(&mut self, source: SettingSource, path: &Path, keys: &[&str], value: Option<Value>) {
        let pos = match self.layers.iter().position(|l| l.source == source) {
            Some(i) => i,
            None => {
                let at = self.layers.iter().position(|l| l.source.rank() > source.rank()).unwrap_or(self.layers.len());
                self.layers.insert(
                    at,
                    SettingsLayer { source, path: Some(path.to_path_buf()), value: Value::Object(Map::new()) },
                );
                at
            }
        };
        set_path(&mut self.layers[pos].value, keys, value);
        self.merged = Value::Object(Map::new());
        for layer in &self.layers {
            deep_merge(&mut self.merged, &layer.value);
        }
    }

    pub fn managed(&self) -> Option<&Value> {
        self.layers.iter().find(|l| l.source == SettingSource::Managed).map(|l| &l.value)
    }
}

/// Write `key = value` into a settings file (creating it), keeping other keys.
/// Set (`Some`) or remove (`None`) the key at `keys` inside a JSON object.
fn set_path(root: &mut Value, keys: &[&str], value: Option<Value>) {
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    let Some((last, parents)) = keys.split_last() else { return };
    let mut cur = root;
    for key in parents {
        if !cur.get(*key).map(Value::is_object).unwrap_or(false) {
            if value.is_none() {
                return;
            }
            cur[*key] = Value::Object(Map::new());
        }
        cur = cur.get_mut(*key).unwrap();
    }
    match value {
        Some(v) => cur[*last] = v,
        None => {
            if let Some(o) = cur.as_object_mut() {
                o.remove(*last);
            }
        }
    }
}

/// A settings file, read for editing. A missing or empty file is `{}`; a file
/// that isn't a JSON object is an error, so a save never wipes what it can't read.
fn read_for_edit(path: &Path) -> std::io::Result<Value> {
    use std::io::{Error, ErrorKind};
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(e) => return Err(e),
    };
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(Error::new(ErrorKind::InvalidData, format!("{} is not a JSON object", path.display()))),
        Err(e) => Err(Error::new(
            ErrorKind::InvalidData,
            format!("{} is not valid JSON ({e}); fix it, then try again", path.display()),
        )),
    }
}

/// Write a settings file whole: a temporary file renamed over it, so a crash
/// or a second writer never leaves it half written. A symlink is followed.
fn write_whole(path: &Path, root: &Value) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(root)? + "\n")?;
    std::fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Remove the key at `pointer` from a settings file. Returns whether it was there.
pub fn remove_setting(path: &Path, pointer: &[&str]) -> std::io::Result<bool> {
    let mut root = read_for_edit(path)?;
    let ptr = format!("/{}", pointer.join("/"));
    if root.pointer(&ptr).is_none() {
        return Ok(false);
    }
    set_path(&mut root, pointer, None);
    write_whole(path, &root)?;
    Ok(true)
}

pub fn write_setting(path: &Path, pointer: &[&str], value: Value) -> std::io::Result<()> {
    let mut root = read_for_edit(path)?;
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
    write_whole(path, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removes_and_finds_overrides() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.json");
        write_setting(&f, &["permissions", "allow"], json!(["Read"])).unwrap();
        write_setting(&f, &["model"], json!("opus")).unwrap();
        assert!(remove_setting(&f, &["model"]).unwrap());
        assert!(!remove_setting(&f, &["model"]).unwrap());
        assert!(!remove_setting(&d.path().join("none.json"), &["model"]).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v, json!({"permissions": {"allow": ["Read"]}}));

        let mut s = LoadedSettings {
            merged: json!({}),
            layers: vec![
                SettingsLayer { source: SettingSource::User, path: None, value: json!({"model": "a"}) },
                SettingsLayer { source: SettingSource::Local, path: None, value: json!({"model": "b"}) },
            ],
            errors: vec![],
        };
        assert_eq!(s.overridden_by(SettingSource::User, "/model").map(|l| l.source), Some(SettingSource::Local));
        assert!(s.overridden_by(SettingSource::Local, "/model").is_none());
        s.apply(SettingSource::Project, Path::new("p.json"), &["effortLevel"], Some(json!("low")));
        assert_eq!(
            s.layers.iter().map(|l| l.source).collect::<Vec<_>>(),
            [SettingSource::User, SettingSource::Project, SettingSource::Local]
        );
        assert_eq!((s.str("/effortLevel"), s.str("/model")), (Some("low"), Some("b")));
        s.apply(SettingSource::Local, Path::new("l.json"), &["model"], None);
        assert_eq!(s.str("/model"), Some("a"));
    }

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

    #[test]
    fn never_overwrites_a_file_it_cannot_read() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.json");
        let broken = "{\"hooks\": {}, // a comment\n \"model\": \"opus\",}\n";
        std::fs::write(&f, broken).unwrap();
        let e = write_setting(&f, &["model"], json!("sonnet")).unwrap_err();
        assert!(e.to_string().contains("not valid JSON"), "{e}");
        assert!(remove_setting(&f, &["model"]).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), broken, "left as it was");
        std::fs::write(&f, "[1]").unwrap();
        assert!(write_setting(&f, &["model"], json!("x")).unwrap_err().to_string().contains("not a JSON object"));
        // Empty and missing files start from {}; no temporary file is left behind.
        std::fs::write(&f, "").unwrap();
        write_setting(&f, &["model"], json!("x")).unwrap();
        write_setting(&d.path().join("new/s.json"), &["model"], json!("y")).unwrap();
        let names: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        #[cfg(unix)]
        {
            // A symlinked settings file (dotfiles) stays a symlink; its target gets the change.
            let real = d.path().join("real.json");
            std::fs::write(&real, "{}").unwrap();
            let link = d.path().join("link.json");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            write_setting(&link, &["model"], json!("z")).unwrap();
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            assert!(std::fs::read_to_string(&real).unwrap().contains("\"z\""));
        }
    }
}
