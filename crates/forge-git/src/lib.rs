//! Thin wrappers over the `git` binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_OPTIONAL_LOCKS", "0").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Top of the working tree containing `dir`.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

pub fn is_repo(dir: &Path) -> bool {
    repo_root(dir).is_some()
}

pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD")
}

/// `origin/HEAD`'s branch, else `main` or `master` when present.
pub fn default_branch(dir: &Path) -> Option<String> {
    if let Some(r) = git(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Some(r.trim_start_matches("origin/").to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|b| git(dir, &["rev-parse", "--verify", "--quiet", b]).is_some())
        .map(str::to_string)
}

/// The repository that owns a linked worktree (its common dir's parent).
pub fn main_repo(dir: &Path) -> Option<PathBuf> {
    let common = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let p = PathBuf::from(common);
    let main = if p.file_name().map(|n| n == ".git").unwrap_or(false) { p.parent()?.to_path_buf() } else { p };
    let root = repo_root(dir)?;
    (main != root).then_some(main)
}

/// A snapshot for the system prompt: branch, default branch, short status, recent commits.
pub fn status_snapshot(dir: &Path) -> Option<String> {
    let root = repo_root(dir)?;
    let branch = current_branch(&root).unwrap_or_else(|| "(detached)".into());
    let main = default_branch(&root).unwrap_or_default();
    let status = git(&root, &["status", "--short"]).unwrap_or_default();
    let status_lines: Vec<&str> = status.lines().collect();
    let status_text = if status_lines.is_empty() {
        "(clean)".to_string()
    } else if status_lines.len() > 40 {
        format!("{}\n... and {} more", status_lines[..40].join("\n"), status_lines.len() - 40)
    } else {
        status
    };
    let log = git(&root, &["log", "--oneline", "-n", "5"]).unwrap_or_default();
    Some(format!(
        "Current branch: {branch}\nMain branch (usually the base for PRs): {main}\n\nStatus:\n{status_text}\n\nRecent commits:\n{log}"
    ))
}

/// Uncommitted changes to tracked files (`git diff HEAD`), and untracked files.
/// `None` outside a repository.
pub fn uncommitted(dir: &Path) -> Option<(String, Vec<String>)> {
    let root = repo_root(dir)?;
    // A repository without commits has no HEAD: diff the index instead.
    let diff = git(&root, &["diff", "HEAD", "--no-color", "--no-ext-diff"])
        .or_else(|| git(&root, &["diff", "--cached", "--no-color", "--no-ext-diff"]))
        .unwrap_or_default();
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    Some((diff, untracked))
}

/// Create a worktree at `path` on a new branch from HEAD.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str) -> Result<(), String> {
    let out = Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg(path)
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// A fingerprint of the working tree's uncommitted state: status, the tracked
/// diff, and the size and mtime of untracked files. Two equal fingerprints mean
/// nothing changed in between, as far as git can see. `None` outside a repository.
pub fn worktree_fingerprint(dir: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let root = repo_root(dir)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    git(&root, &["status", "--porcelain=v1", "--untracked-files=all"])?.hash(&mut h);
    git(&root, &["diff", "--no-ext-diff", "--no-color", "HEAD"]).unwrap_or_default().hash(&mut h);
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    for f in untracked.lines().take(5000) {
        if let Ok(m) = std::fs::metadata(root.join(f)) {
            (f, m.len(), m.modified().ok()).hash(&mut h);
        }
    }
    Some(h.finish())
}

/// Add `pattern` to the repository's `.git/info/exclude` (untracked, local ignore) if missing.
pub fn exclude(repo_root: &Path, pattern: &str) -> std::io::Result<()> {
    let git_dir = repo_root.join(".git");
    if !git_dir.is_dir() {
        return Ok(());
    }
    let path = git_dir.join("info").join("exclude");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().expect("info dir"))?;
    let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&path, format!("{current}{sep}{pattern}\n"))
}

/// Uncommitted changes in a working tree?
pub fn is_dirty(dir: &Path) -> bool {
    git(dir, &["status", "--porcelain"]).map(|s| !s.is_empty()).unwrap_or(false)
}

pub fn remove_worktree(repo: &Path, path: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .args(["worktree", "remove"])
        .arg(path)
        .current_dir(repo)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "first"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(dir).status().unwrap().success());
        }
    }

    #[test]
    fn snapshot_and_worktree() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        init(&root);
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let snap = status_snapshot(&root).unwrap();
        assert!(snap.contains("Current branch: main") && snap.contains("?? a.txt") && snap.contains("first"), "{snap}");
        let wt = root.join(".forge/worktrees/w1");
        add_worktree(&root, &wt, "forge/w1").unwrap();
        assert_eq!(current_branch(&wt).as_deref(), Some("forge/w1"));
        assert_eq!(main_repo(&wt), Some(root.clone()));
        assert_eq!(main_repo(&root), None);
        assert!(!is_dirty(&wt));
        remove_worktree(&root, &wt).unwrap();
        assert!(status_snapshot(Path::new("/")).is_none());
    }

    #[test]
    fn fingerprint_moves_with_the_worktree() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(worktree_fingerprint(d.path()), None);
        let root = d.path().canonicalize().unwrap();
        init(&root);
        let clean = worktree_fingerprint(&root).unwrap();
        assert_eq!(worktree_fingerprint(&root), Some(clean), "stable when nothing changes");
        std::fs::write(root.join("new.txt"), "a").unwrap();
        let untracked = worktree_fingerprint(&root).unwrap();
        assert_ne!(untracked, clean);
        std::fs::write(root.join("new.txt"), "ab").unwrap();
        assert_ne!(worktree_fingerprint(&root).unwrap(), untracked, "an edited untracked file counts");
    }
}
