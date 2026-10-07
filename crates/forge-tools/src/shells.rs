//! Running shell commands: foreground with timeout / interrupt, and
//! background shells that outlive the call (BashOutput / KillShell).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge_platform::process::ProcessTree;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

pub use forge_platform::shell::{Shell, ShellChoice, ShellKind, ShellMissing};

/// Why a command didn't start.
#[derive(Debug)]
pub enum StartError {
    /// No usable shell in this session (the Bash tool reports it as non-retryable).
    NoShell(ShellMissing),
    /// The shell (or the sandbox wrapper) exists but could not be started.
    Spawn { program: String, error: std::io::Error },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NoShell(m) => write!(f, "{m}"),
            StartError::Spawn { program, error } => write!(f, "could not start {program}: {error}"),
        }
    }
}

impl StartError {
    /// No command can run until the user fixes the setup.
    pub fn is_setup_problem(&self) -> bool {
        match self {
            StartError::NoShell(_) => true,
            StartError::Spawn { error, .. } => error.kind() == std::io::ErrorKind::NotFound,
        }
    }
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

type Sandbox<'a> = Option<&'a (crate::sandbox::Backend, crate::sandbox::SandboxPolicy)>;

struct Prepared {
    command: Command,
    program: String,
    /// Windows: the script file, kept until the process exits.
    script: Option<forge_platform::shell::ScriptFile>,
}

fn build(
    shell: &ShellChoice,
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    cwd_file: Option<&Path>,
    sandbox: Sandbox,
) -> Result<Prepared, StartError> {
    let shell = shell.as_ref().map_err(|m| StartError::NoShell(m.clone()))?;
    let script = shell.script(command, cwd_file);
    let program = shell.program.display().to_string();
    let (mut c, program, file) = match sandbox {
        Some((backend, policy)) => {
            let (prog, args) = policy.wrap(*backend, &program, &script, cwd);
            let mut c = std::process::Command::new(&prog);
            c.args(args);
            (c, prog, None)
        }
        None => {
            let (c, file) =
                shell.command(&script).map_err(|error| StartError::Spawn { program: program.clone(), error })?;
            (c, program, file)
        }
    };
    c.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.env("FORGECLI", "1");
    c.envs(env);
    forge_platform::process::isolate(&mut c);
    let mut c = Command::from(c);
    c.kill_on_drop(true);
    Ok(Prepared { command: c, program, script: file })
}

fn spawn(p: &mut Prepared) -> Result<(Child, Option<ProcessTree>), StartError> {
    let child = p.command.spawn().map_err(|error| StartError::Spawn { program: p.program.clone(), error })?;
    let tree = child.id().map(ProcessTree::attach);
    Ok((child, tree))
}

/// Output kept from each stream: the first and the last this many bytes.
const KEEP_BYTES: usize = 4 << 20;

/// Read a stream to its end, keeping its start and end: a command that prints
/// gigabytes can't fill memory.
async fn read_all<R: AsyncRead + Unpin>(mut r: R) -> String {
    read_capped(&mut r, KEEP_BYTES).await
}

async fn read_capped<R: AsyncRead + Unpin>(r: &mut R, keep: usize) -> String {
    let mut head: Vec<u8> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let mut dropped: u64 = 0;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut data = &chunk[..n];
        if head.len() < keep {
            let take = data.len().min(keep - head.len());
            head.extend_from_slice(&data[..take]);
            data = &data[take..];
        }
        tail.extend_from_slice(data);
        if tail.len() > 2 * keep {
            let cut = tail.len() - keep;
            dropped += cut as u64;
            tail.drain(..cut);
        }
    }
    if tail.len() > keep {
        let cut = tail.len() - keep;
        dropped += cut as u64;
        tail.drain(..cut);
    }
    let mut out = String::from_utf8_lossy(&head).into_owned();
    if dropped > 0 {
        out.push_str(&format!("\n[... {dropped} bytes of output omitted ...]\n"));
    }
    out.push_str(&String::from_utf8_lossy(&tail));
    out
}

/// Run a command to completion, honouring timeout and cancellation.
pub async fn run_command(
    shell: &ShellChoice,
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    timeout: Duration,
    cancel: &CancellationToken,
    sandbox: Sandbox<'_>,
) -> Result<CommandResult, StartError> {
    let cwd_file = std::env::temp_dir().join(format!("forge-cwd-{}", uuid::Uuid::new_v4()));
    let mut prepared = build(shell, command, cwd, env, Some(&cwd_file), sandbox)?;
    let (mut child, tree) = spawn(&mut prepared)?;
    let kill = || {
        if let Some(t) = &tree {
            t.kill();
        }
    };
    let out = tokio::spawn(read_all(child.stdout.take().expect("piped")));
    let err = tokio::spawn(read_all(child.stderr.take().expect("piped")));
    let mut result = CommandResult::default();
    tokio::select! {
        status = child.wait() => { result.code = status.ok().and_then(|s| s.code()); }
        _ = tokio::time::sleep(timeout) => { result.timed_out = true; kill(); let _ = child.wait().await; }
        _ = cancel.cancelled() => { result.interrupted = true; kill(); let _ = child.wait().await; }
    }
    // Background children of the command may keep the pipes open; do not wait forever.
    let grace = Duration::from_millis(if result.timed_out || result.interrupted { 200 } else { 2000 });
    result.stdout = tokio::time::timeout(grace, out).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    result.stderr = tokio::time::timeout(grace, err).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    if let (Ok(written), Ok(shell)) = (std::fs::read_to_string(&cwd_file), shell) {
        result.final_cwd = shell.parse_cwd(&written);
    }
    if cfg!(windows) {
        result.stdout = result.stdout.replace("\r\n", "\n");
        result.stderr = result.stderr.replace("\r\n", "\n");
    }
    drop(prepared);
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
    tree: Option<ProcessTree>,
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
            if let Some(t) = &self.tree {
                t.kill();
            }
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
        shell: &ShellChoice,
        command: &str,
        cwd: &Path,
        env: &HashMap<String, String>,
        sandbox: Sandbox<'_>,
    ) -> Result<Arc<BackgroundShell>, StartError> {
        let mut prepared = build(shell, command, cwd, env, None, sandbox)?;
        prepared.command.kill_on_drop(false);
        let (mut child, tree) = spawn(&mut prepared)?;
        let script = prepared.script.take();
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
            tree,
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
            drop(script);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn long_output_keeps_its_start_and_end() {
        let data: Vec<u8> = (0..1000u32).flat_map(|i| format!("{i:04}\n").into_bytes()).collect();
        let out = read_capped(&mut data.as_slice(), 100).await;
        assert!(out.starts_with("0000\n0001\n"), "{out}");
        assert!(out.ends_with("0998\n0999\n"), "{out}");
        assert!(out.contains("[... 4800 bytes of output omitted ...]"), "{out}");
        let short = read_capped(&mut b"hi\n".as_slice(), 100).await;
        assert_eq!(short, "hi\n");
    }
}
