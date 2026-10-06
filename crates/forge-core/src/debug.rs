//! `/debug`: turn on debug logging partway through a session. The front end
//! owns the logger, so it registers how to switch it on.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

type Enabler = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

static ENABLER: OnceLock<Enabler> = OnceLock::new();
/// Where logs go: set at startup (`--debug`, `FORGE_LOG`) or by `/debug`.
static ACTIVE: Mutex<Option<String>> = Mutex::new(None);

/// Register how to start logging to a file (once, by the front end).
pub fn set_enabler(f: impl Fn(&Path) -> Result<(), String> + Send + Sync + 'static) {
    let _ = ENABLER.set(Box::new(f));
}

/// Logging was configured at startup; `to` says where it goes.
pub fn set_startup(to: impl Into<String>) {
    *ACTIVE.lock().unwrap() = Some(to.into());
}

/// Where logs are going, if logging is on.
pub fn active() -> Option<String> {
    ACTIVE.lock().unwrap().clone()
}

/// This session's debug log.
pub fn log_path(session_id: &str) -> PathBuf {
    forge_config::state_dir().join("debug").join(format!("{session_id}.txt"))
}

/// Start debug logging to `path`.
pub fn enable(path: &Path) -> Result<(), String> {
    let f = ENABLER.get().ok_or("this front end can't turn on debug logging")?;
    f(path)?;
    *ACTIVE.lock().unwrap() = Some(path.display().to_string());
    Ok(())
}
