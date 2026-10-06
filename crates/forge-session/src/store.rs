use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{sha_hex, validate_session_id, SessionError};

/// `$FORGE_HOME`, else `~/.forge`.
pub fn forge_home() -> PathBuf {
    std::env::var_os("FORGE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".forge"))
}

/// Readable directory key for a project (contract C5).
pub fn project_key(cwd: &Path) -> String {
    let key: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    if key.len() <= 200 {
        key
    } else {
        format!("{}-{}", &key[..180], &sha_hex(&cwd.to_string_lossy())[..12])
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
    pub modified: SystemTime,
    pub first_prompt: String,
    pub title: Option<String>,
    pub message_count: usize,
    pub git_branch: Option<String>,
    pub worktree: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    /// project key -> canonical cwds that map to it
    projects: BTreeMap<String, Vec<PathBuf>>,
    /// worktree cwd -> main repo
    #[serde(default)]
    main_repo: BTreeMap<PathBuf, PathBuf>,
}

/// `~/.forge/projects`.
#[derive(Debug, Clone)]
pub struct SessionStore {
    pub root: PathBuf,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        SessionStore { root }
    }

    pub fn default_store() -> Self {
        SessionStore::new(forge_home().join("projects"))
    }

    pub fn project_dir(&self, cwd: &Path) -> PathBuf {
        self.root.join(project_key(cwd))
    }

    pub fn session_path(&self, cwd: &Path, id: &str) -> Result<PathBuf, SessionError> {
        validate_session_id(id)?;
        Ok(self.project_dir(cwd).join(format!("{id}.jsonl")))
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn read_index(&self) -> Index {
        std::fs::read_to_string(self.index_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Record that `cwd` uses its key (and, for a worktree, its main repo).
    pub fn register_project(&self, cwd: &Path, main_repo: Option<&Path>) -> Result<(), SessionError> {
        std::fs::create_dir_all(&self.root)?;
        let mut idx = self.read_index();
        let list = idx.projects.entry(project_key(cwd)).or_default();
        if !list.iter().any(|p| p == cwd) {
            list.push(cwd.to_path_buf());
        }
        if let Some(m) = main_repo {
            idx.main_repo.insert(cwd.to_path_buf(), m.to_path_buf());
        }
        let tmp = self.index_path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&idx).unwrap_or_default())?;
        std::fs::rename(tmp, self.index_path())?;
        Ok(())
    }

    /// Sessions for exactly `cwd` (and for its worktrees when it is a main repo), newest first.
    pub fn list(&self, cwd: &Path) -> Vec<SessionSummary> {
        let idx = self.read_index();
        let mut dirs: Vec<PathBuf> = vec![cwd.to_path_buf()];
        for (wt, main) in &idx.main_repo {
            if main == cwd {
                dirs.push(wt.clone());
            }
        }
        let mut out = vec![];
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(self.project_dir(&d)) else { continue };
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Some(s) = summarize(&path) {
                    // Keys can collide: keep only sessions recorded for this exact cwd.
                    if s.cwd == d {
                        out.push(s);
                    }
                }
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.modified));
        out
    }

    /// Most recent session in `cwd`.
    pub fn latest(&self, cwd: &Path) -> Option<SessionSummary> {
        self.list(cwd).into_iter().find(|s| s.cwd == cwd)
    }

    /// Find a session by id in any project.
    pub fn find(&self, id: &str) -> Result<PathBuf, SessionError> {
        validate_session_id(id)?;
        let Ok(rd) = std::fs::read_dir(&self.root) else { return Err(SessionError::NotFound(id.into())) };
        for e in rd.flatten() {
            let p = e.path().join(format!("{id}.jsonl"));
            if p.exists() {
                return Ok(p);
            }
        }
        Err(SessionError::NotFound(id.into()))
    }
}

/// Read enough of a transcript to list it.
fn summarize(path: &Path) -> Option<SessionSummary> {
    let text = std::fs::read_to_string(path).ok()?;
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut s = SessionSummary {
        session_id: path.file_stem()?.to_string_lossy().into_owned(),
        path: path.to_path_buf(),
        cwd: PathBuf::new(),
        modified,
        first_prompt: String::new(),
        title: None,
        message_count: 0,
        git_branch: None,
        worktree: None,
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if s.cwd.as_os_str().is_empty() {
            if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                s.cwd = PathBuf::from(c);
            }
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") | Some("assistant") => {
                s.message_count += 1;
                if s.first_prompt.is_empty() && v["type"] == "user" && v.get("isMeta").is_none() {
                    let content = &v["message"]["content"];
                    let text = content.as_str().map(str::to_string).or_else(|| {
                        content.as_array().and_then(|a| {
                            a.iter().find_map(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_string))
                        })
                    });
                    if let Some(t) = text {
                        s.first_prompt = t.chars().take(200).collect();
                    }
                }
                if s.git_branch.is_none() {
                    s.git_branch = v.get("gitBranch").and_then(|b| b.as_str()).map(str::to_string);
                }
            }
            Some("title") => s.title = v.get("title").and_then(|t| t.as_str()).map(str::to_string),
            Some("session_meta") => {
                if let Some(w) = v.pointer("/worktree/name").and_then(|w| w.as_str()) {
                    s.worktree = Some(w.to_string());
                }
            }
            _ => {}
        }
    }
    (s.message_count > 0).then_some(s)
}
