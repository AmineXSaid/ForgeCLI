use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Which files the model has read, and when: Edit and Write refuse to touch
/// an existing file the model has not read, or that changed since.
#[derive(Debug, Default)]
pub struct FileState {
    reads: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
    /// What each Read showed: (path, offset, limit) -> (content hash, tool_use_id).
    views: Mutex<HashMap<ViewKey, (u64, String)>>,
}

type ViewKey = (PathBuf, usize, Option<usize>);

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl FileState {
    pub fn record_read(&self, path: &Path) {
        self.reads.lock().unwrap().insert(path.to_path_buf(), mtime(path));
    }

    /// Call after ForgeCLI itself writes the file, so the next edit is allowed.
    pub fn record_write(&self, path: &Path) {
        self.record_read(path);
    }

    pub fn forget(&self, path: &Path) {
        self.reads.lock().unwrap().remove(path);
    }

    /// The earlier Read (its tool_use_id) that showed this exact range of
    /// this exact content, if its result is still in the conversation.
    pub fn same_view(&self, path: &Path, offset: usize, limit: Option<usize>, hash: u64) -> Option<String> {
        let views = self.views.lock().unwrap();
        views.get(&(path.to_path_buf(), offset, limit)).filter(|(h, _)| *h == hash).map(|(_, id)| id.clone())
    }

    pub fn record_view(&self, path: &Path, offset: usize, limit: Option<usize>, hash: u64, tool_use_id: &str) {
        self.views.lock().unwrap().insert((path.to_path_buf(), offset, limit), (hash, tool_use_id.to_string()));
    }

    /// Results cleared from the conversation (micro-compaction) no longer count as seen.
    pub fn forget_views(&self, tool_use_ids: &[String]) {
        self.views.lock().unwrap().retain(|_, (_, id)| !tool_use_ids.contains(id));
    }

    /// After a full compaction nothing earlier is in view.
    pub fn forget_all_views(&self) {
        self.views.lock().unwrap().clear();
    }

    /// `Ok` when an existing file may be modified.
    pub fn check_writable(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        match self.reads.lock().unwrap().get(path) {
            None => Err("File has not been read yet. Read it first before writing to it.".into()),
            Some(seen) if *seen != mtime(path) => Err(
                "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
                    .into(),
            ),
            Some(_) => Ok(()),
        }
    }
}
