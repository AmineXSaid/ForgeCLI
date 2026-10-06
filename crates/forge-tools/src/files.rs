use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Which files the model has read, and when: Edit and Write refuse to touch
/// an existing file the model has not read, or that changed since.
#[derive(Debug, Default)]
pub struct FileState {
    reads: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
}

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
