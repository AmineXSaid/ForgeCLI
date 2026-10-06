use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::files::FileState;
use crate::shells::ShellManager;

/// Saves a file's prior content before a tool writes it (contract C4).
pub trait Checkpointer: Send + Sync {
    fn before_write(&self, path: &Path);
}

/// Everything a tool call may use. Cheap to clone; shared state sits behind `Arc`s.
#[derive(Clone)]
pub struct ToolContext {
    /// The session's project directory (where `forge` was started).
    pub project_dir: PathBuf,
    /// The Bash tool's current directory; persists between calls.
    pub shell_cwd: Arc<Mutex<PathBuf>>,
    /// Project dir plus `--add-dir` directories.
    pub working_dirs: Arc<RwLock<Vec<PathBuf>>>,
    pub cancel: CancellationToken,
    pub session_id: String,
    pub tool_use_id: String,
    pub files: Arc<FileState>,
    pub checkpointer: Option<Arc<dyn Checkpointer>>,
    pub shells: Arc<ShellManager>,
    pub todos: Arc<Mutex<Vec<Value>>>,
    /// Extra environment for spawned commands.
    pub env: Arc<HashMap<String, String>>,
    /// Upper bound for Bash output kept in a result.
    pub max_output_chars: usize,
    /// OS sandbox for shell commands (`None` = commands run unconfined).
    pub sandbox: Option<Arc<crate::sandbox::SandboxPolicy>>,
}

impl ToolContext {
    pub fn new(project_dir: &Path) -> Self {
        ToolContext {
            project_dir: project_dir.to_path_buf(),
            shell_cwd: Arc::new(Mutex::new(project_dir.to_path_buf())),
            working_dirs: Arc::new(RwLock::new(vec![project_dir.to_path_buf()])),
            cancel: CancellationToken::new(),
            session_id: String::new(),
            tool_use_id: String::new(),
            files: Arc::new(FileState::default()),
            checkpointer: None,
            shells: Arc::new(ShellManager::default()),
            todos: Arc::new(Mutex::new(vec![])),
            env: Arc::new(HashMap::new()),
            max_output_chars: 30_000,
            sandbox: None,
        }
    }

    /// A per-call copy with its own id and cancellation.
    pub fn for_call(&self, tool_use_id: &str, cancel: CancellationToken) -> Self {
        let mut c = self.clone();
        c.tool_use_id = tool_use_id.to_string();
        c.cancel = cancel;
        c
    }

    pub fn shell_cwd(&self) -> PathBuf {
        self.shell_cwd.lock().unwrap().clone()
    }

    pub fn in_working_dirs(&self, p: &Path) -> bool {
        let p = forge_permissions::normalize(p, &self.project_dir);
        self.working_dirs.read().unwrap().iter().any(|d| p.starts_with(d))
    }

    /// The sandbox policy for a command now, with the current working directories writable.
    pub fn sandbox_now(&self) -> Option<(crate::sandbox::Backend, crate::sandbox::SandboxPolicy)> {
        let policy = self.sandbox.as_ref()?;
        let backend = crate::sandbox::backend()?;
        let mut p = (**policy).clone();
        p.writable_roots = self.working_dirs.read().unwrap().clone();
        Some((backend, p))
    }

    pub fn checkpoint(&self, path: &Path) {
        if let Some(c) = &self.checkpointer {
            c.before_write(path);
        }
    }
}
