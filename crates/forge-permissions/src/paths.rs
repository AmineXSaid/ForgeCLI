use std::path::{Component, Path, PathBuf};

/// Absolute, lexically normalised path (no `.`/`..`), without touching the
/// filesystem, so it works for files that do not exist yet. `~` expands to
/// the home directory.
pub fn normalize(path: &Path, base: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let joined = if let Some(rest) = s.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else if s == "~" {
        dirs::home_dir().unwrap_or_default()
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` equals `dir` or lies below it (both already normalised).
pub fn is_within(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir)
}
