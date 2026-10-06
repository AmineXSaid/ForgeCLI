//! File checkpoints (contract C4): content snapshots taken before the first
//! write to a file in each user turn, restored by rewind.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::sha_hex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Record {
    turn: String,
    path: PathBuf,
    /// Snapshot version, or `None` when the file did not exist.
    version: Option<u32>,
}

#[derive(Debug, Default)]
struct State {
    turns: Vec<String>,
    current: Option<String>,
    versions: HashMap<PathBuf, u32>,
    this_turn: HashSet<PathBuf>,
    /// Every write this turn, in order (repeats included), for the verification loop.
    writes: Vec<PathBuf>,
    records: Vec<Record>,
}

/// What a rewind would do.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RewindPlan {
    pub restore: Vec<PathBuf>,
    pub delete: Vec<PathBuf>,
}

impl RewindPlan {
    pub fn is_empty(&self) -> bool {
        self.restore.is_empty() && self.delete.is_empty()
    }
}

pub struct FileHistory {
    dir: PathBuf,
    state: Mutex<State>,
}

impl FileHistory {
    /// History for a session under `~/.forge/file-history/<session-id>`.
    pub fn new(dir: PathBuf) -> Self {
        let mut st = State::default();
        if let Ok(text) = std::fs::read_to_string(dir.join("index.jsonl")) {
            for r in text.lines().filter_map(|l| serde_json::from_str::<Record>(l).ok()) {
                if !st.turns.contains(&r.turn) {
                    st.turns.push(r.turn.clone());
                }
                if let Some(v) = r.version {
                    let e = st.versions.entry(r.path.clone()).or_insert(0);
                    *e = (*e).max(v);
                }
                st.records.push(r);
            }
        }
        FileHistory { dir, state: Mutex::new(st) }
    }

    pub fn for_session(session_id: &str) -> Self {
        Self::new(crate::forge_home().join("file-history").join(session_id))
    }

    /// Start a user turn; `turn` is the user message's transcript uuid.
    pub fn begin_turn(&self, turn: &str) {
        let mut st = self.state.lock().unwrap();
        if !st.turns.iter().any(|t| t == turn) {
            st.turns.push(turn.to_string());
        }
        st.current = Some(turn.to_string());
        st.this_turn.clear();
        st.writes.clear();
    }

    /// Number of file writes so far this turn.
    pub fn writes_len(&self) -> usize {
        self.state.lock().unwrap().writes.len()
    }

    /// Files written this turn after the first `mark` writes, first write first, without repeats.
    pub fn writes_since(&self, mark: usize) -> Vec<PathBuf> {
        let st = self.state.lock().unwrap();
        let mut out: Vec<PathBuf> = vec![];
        for p in st.writes.iter().skip(mark) {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    }

    fn snapshot_path(&self, path: &Path, version: u32) -> PathBuf {
        self.dir.join(format!("{}@v{version}", &sha_hex(&path.to_string_lossy())[..16]))
    }

    /// Snapshot `path` if this turn has not yet.
    pub fn snapshot(&self, path: &Path) {
        let mut st = self.state.lock().unwrap();
        let Some(turn) = st.current.clone() else { return };
        st.writes.push(path.to_path_buf());
        if !st.this_turn.insert(path.to_path_buf()) {
            return;
        }
        let _ = std::fs::create_dir_all(&self.dir);
        let version = if path.exists() {
            let v = st.versions.get(path).copied().unwrap_or(0) + 1;
            if let Err(e) = std::fs::copy(path, self.snapshot_path(path, v)) {
                tracing::warn!(error = %e, path = %path.display(), "checkpoint failed");
                return;
            }
            st.versions.insert(path.to_path_buf(), v);
            Some(v)
        } else {
            None
        };
        let rec = Record { turn, path: path.to_path_buf(), version };
        if let Ok(line) = serde_json::to_string(&rec) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("index.jsonl"))
            {
                let _ = writeln!(f, "{line}");
            }
        }
        st.records.push(rec);
    }

    /// Plan (and unless `dry_run`, apply) restoring files to how they were
    /// before user turn `turn`.
    pub fn rewind(&self, turn: &str, dry_run: bool) -> Result<RewindPlan, String> {
        let st = self.state.lock().unwrap();
        let Some(pos) = st.turns.iter().position(|t| t == turn) else {
            return Ok(RewindPlan::default());
        };
        let later: HashSet<&String> = st.turns[pos..].iter().collect();
        let mut first: Vec<&Record> = vec![];
        for r in st.records.iter().filter(|r| later.contains(&r.turn)) {
            if !first.iter().any(|f| f.path == r.path) {
                first.push(r);
            }
        }
        let mut plan = RewindPlan::default();
        for r in &first {
            match r.version {
                Some(_) => plan.restore.push(r.path.clone()),
                None => {
                    if r.path.exists() {
                        plan.delete.push(r.path.clone())
                    }
                }
            }
        }
        if dry_run {
            return Ok(plan);
        }
        for r in first {
            match r.version {
                Some(v) => {
                    if let Some(parent) = r.path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::copy(self.snapshot_path(&r.path, v), &r.path)
                        .map_err(|e| format!("restore {}: {e}", r.path.display()))?;
                }
                None => {
                    if r.path.exists() {
                        std::fs::remove_file(&r.path).map_err(|e| format!("delete {}: {e}", r.path.display()))?;
                    }
                }
            }
        }
        Ok(plan)
    }

    /// The files each user turn changed, oldest turn first (`/diff`).
    pub fn turns(&self) -> Vec<(String, Vec<PathBuf>)> {
        let st = self.state.lock().unwrap();
        st.turns
            .iter()
            .map(|t| {
                let mut files: Vec<PathBuf> = vec![];
                for r in st.records.iter().filter(|r| &r.turn == t) {
                    if !files.contains(&r.path) {
                        files.push(r.path.clone());
                    }
                }
                (t.clone(), files)
            })
            .filter(|(_, f)| !f.is_empty())
            .collect()
    }

    /// Each changed file's content before the session first changed it
    /// (`None`: the file didn't exist), in the order they were first changed.
    pub fn originals(&self) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let st = self.state.lock().unwrap();
        let mut out: Vec<(PathBuf, Option<Vec<u8>>)> = vec![];
        for r in &st.records {
            if out.iter().any(|(p, _)| p == &r.path) {
                continue;
            }
            let before = r.version.and_then(|v| std::fs::read(self.snapshot_path(&r.path, v)).ok());
            out.push((r.path.clone(), before));
        }
        out
    }

    /// Files checkpointed at or after `turn` (for the rewind confirmation).
    pub fn changed_since(&self, turn: &str) -> Vec<PathBuf> {
        self.rewind(turn, true).map(|p| p.restore.into_iter().chain(p.delete).collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_turns_and_originals() {
        let d = tempfile::tempdir().unwrap();
        let h = FileHistory::new(d.path().join("hist"));
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, "v0").unwrap();
        h.begin_turn("t1");
        h.snapshot(&a);
        std::fs::write(&a, "v1").unwrap();
        h.begin_turn("t2");
        h.snapshot(&a);
        h.snapshot(&b);
        std::fs::write(&b, "new").unwrap();
        h.begin_turn("t3");
        assert_eq!(
            h.turns(),
            vec![("t1".to_string(), vec![a.clone()]), ("t2".to_string(), vec![a.clone(), b.clone()])]
        );
        assert_eq!(h.originals(), vec![(a, Some(b"v0".to_vec())), (b, None)]);
    }

    #[test]
    fn rewind_restores_and_deletes() {
        let d = tempfile::tempdir().unwrap();
        let h = FileHistory::new(d.path().join("hist"));
        let a = d.path().join("a.txt");
        let b = d.path().join("b.txt");
        std::fs::write(&a, "v0").unwrap();

        h.begin_turn("t1");
        h.snapshot(&a);
        std::fs::write(&a, "v1").unwrap();
        h.snapshot(&a); // second write in the same turn: no new snapshot
        std::fs::write(&a, "v1b").unwrap();

        h.begin_turn("t2");
        h.snapshot(&a);
        std::fs::write(&a, "v2").unwrap();
        h.snapshot(&b);
        std::fs::write(&b, "new").unwrap();

        let plan = h.rewind("t2", true).unwrap();
        assert_eq!(plan.restore, vec![a.clone()]);
        assert_eq!(plan.delete, vec![b.clone()]);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "v2", "dry run changes nothing");

        h.rewind("t2", false).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "v1b");
        assert!(!b.exists());

        // Survives a restart.
        let h2 = FileHistory::new(d.path().join("hist"));
        h2.rewind("t1", false).unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "v0");
    }
}
