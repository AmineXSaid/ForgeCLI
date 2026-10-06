//! Running shell commands: foreground with timeout / interrupt, and
//! background shells that outlive the call (BashOutput / KillShell).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

/// The shell used for commands: bash when available, else sh.
pub fn shell_program() -> String {
    for candidate in ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "/bin/sh".to_string()
}

#[derive(Debug, Clone, Default)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
    pub interrupted: bool,
    pub timed_out: bool,
    /// Directory the command finished in, when it could be determined.
    pub final_cwd: Option<PathBuf>,
}

fn build(command: &str, cwd: &Path, env: &HashMap<String, String>, cwd_file: Option<&Path>) -> Command {
    let script = match cwd_file {
        Some(f) => format!(
            "{command}\n__forge_ec=$?\npwd -P > '{}' 2>/dev/null\nexit $__forge_ec",
            f.display().to_string().replace('\'', "'\\''")
        ),
        None => command.to_string(),
    };
    let mut c = Command::new(shell_program());
    c.arg("-c").arg(script).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.env("FORGECLI", "1");
    for (k, v) in env {
        c.env(k, v);
    }
    #[cfg(unix)]
    c.process_group(0);
    c.kill_on_drop(true);
    c
}

/// Kill the whole process group (the shell and everything it started).
pub fn kill_tree(child_pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = child_pid {
        // SAFETY: plain syscall; a negative pid addresses the process group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        });
    }
    #[cfg(not(unix))]
    let _ = child_pid;
}

async fn read_all<R: AsyncRead + Unpin>(mut r: R) -> String {
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

/// Run a command to completion, honouring timeout and cancellation.
pub async fn run_command(
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    timeout: Duration,
    cancel: &CancellationToken,
) -> std::io::Result<CommandResult> {
    let cwd_file = std::env::temp_dir().join(format!("forge-cwd-{}", uuid::Uuid::new_v4()));
    let mut child: Child = build(command, cwd, env, Some(&cwd_file)).spawn()?;
    let pid = child.id();
    let out = tokio::spawn(read_all(child.stdout.take().expect("piped")));
    let err = tokio::spawn(read_all(child.stderr.take().expect("piped")));
    let mut result = CommandResult::default();
    tokio::select! {
        status = child.wait() => { result.code = status.ok().and_then(|s| s.code()); }
        _ = tokio::time::sleep(timeout) => { result.timed_out = true; kill_tree(pid); let _ = child.wait().await; }
        _ = cancel.cancelled() => { result.interrupted = true; kill_tree(pid); let _ = child.wait().await; }
    }
    // Background children of the command may keep the pipes open; do not wait forever.
    let grace = Duration::from_millis(if result.timed_out || result.interrupted { 200 } else { 2000 });
    result.stdout = tokio::time::timeout(grace, out).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    result.stderr = tokio::time::timeout(grace, err).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    if let Ok(dir) = std::fs::read_to_string(&cwd_file) {
        let dir = dir.trim();
        if !dir.is_empty() {
            result.final_cwd = Some(PathBuf::from(dir));
        }
    }
    let _ = std::fs::remove_file(&cwd_file);
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShellStatus {
    Running,
    Completed(Option<i32>),
    Killed,
}

impl ShellStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ShellStatus::Running => "running",
            ShellStatus::Completed(_) => "completed",
            ShellStatus::Killed => "killed",
        }
    }
}

#[derive(Debug)]
pub struct BackgroundShell {
    pub id: String,
    pub command: String,
    pub started: Instant,
    stdout: Mutex<String>,
    stderr: Mutex<String>,
    read_out: Mutex<usize>,
    read_err: Mutex<usize>,
    status: Mutex<ShellStatus>,
    pid: Option<u32>,
}

impl BackgroundShell {
    pub fn status(&self) -> ShellStatus {
        self.status.lock().unwrap().clone()
    }

    /// Output produced since the previous call.
    pub fn take_new_output(&self) -> (String, String) {
        let take = |buf: &Mutex<String>, pos: &Mutex<usize>| {
            let b = buf.lock().unwrap();
            let mut p = pos.lock().unwrap();
            let s = b[*p..].to_string();
            *p = b.len();
            s
        };
        (take(&self.stdout, &self.read_out), take(&self.stderr, &self.read_err))
    }

    pub fn kill(&self) {
        let mut st = self.status.lock().unwrap();
        if *st == ShellStatus::Running {
            kill_tree(self.pid);
            *st = ShellStatus::Killed;
        }
    }
}

/// Background shells of a session.
#[derive(Debug, Default)]
pub struct ShellManager {
    shells: Mutex<HashMap<String, Arc<BackgroundShell>>>,
    counter: AtomicU32,
}

impl ShellManager {
    pub fn spawn(
        &self,
        command: &str,
        cwd: &Path,
        env: &HashMap<String, String>,
    ) -> std::io::Result<Arc<BackgroundShell>> {
        let mut child = build(command, cwd, env, None).kill_on_drop(false).spawn()?;
        let id = format!("bash_{}", self.counter.fetch_add(1, Ordering::SeqCst) + 1);
        let shell = Arc::new(BackgroundShell {
            id: id.clone(),
            command: command.to_string(),
            started: Instant::now(),
            stdout: Mutex::new(String::new()),
            stderr: Mutex::new(String::new()),
            read_out: Mutex::new(0),
            read_err: Mutex::new(0),
            status: Mutex::new(ShellStatus::Running),
            pid: child.id(),
        });
        let pump = |mut r: Box<dyn AsyncRead + Unpin + Send>, sh: Arc<BackgroundShell>, is_err: bool| async move {
            let mut buf = [0u8; 8192];
            loop {
                match r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        let target = if is_err { &sh.stderr } else { &sh.stdout };
                        target.lock().unwrap().push_str(&text);
                    }
                }
            }
        };
        tokio::spawn(pump(Box::new(child.stdout.take().expect("piped")), shell.clone(), false));
        tokio::spawn(pump(Box::new(child.stderr.take().expect("piped")), shell.clone(), true));
        let sh = shell.clone();
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            let mut st = sh.status.lock().unwrap();
            if *st == ShellStatus::Running {
                *st = ShellStatus::Completed(code);
            }
        });
        self.shells.lock().unwrap().insert(id, shell.clone());
        Ok(shell)
    }

    pub fn get(&self, id: &str) -> Option<Arc<BackgroundShell>> {
        self.shells.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<BackgroundShell>> {
        let mut v: Vec<_> = self.shells.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|s| s.started);
        v
    }

    /// Kill every running shell (session end).
    pub fn kill_all(&self) {
        for s in self.list() {
            s.kill();
        }
    }
}
