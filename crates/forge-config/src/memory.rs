//! Memory files: instructions loaded into every conversation.

use std::path::{Path, PathBuf};

use crate::{claude_home, forge_home, home};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    User,
    Project,
    Local,
}

#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub kind: MemoryKind,
    pub content: String,
}

const MAX_IMPORT_DEPTH: usize = 5;

/// Expand `@path` imports (outside code fences) recursively.
fn expand_imports(content: &str, dir: &Path, depth: usize, seen: &mut Vec<PathBuf>) -> String {
    if depth >= MAX_IMPORT_DEPTH {
        return content.to_string();
    }
    let mut out = String::new();
    let mut in_fence = false;
    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        let t = line.trim();
        if !in_fence && t.starts_with('@') && !t.contains(' ') && t.len() > 1 {
            let raw = &t[1..];
            let path = if let Some(rest) = raw.strip_prefix("~/") { home().join(rest) } else { dir.join(raw) };
            if let Ok(canon) = path.canonicalize() {
                if !seen.contains(&canon) {
                    if let Ok(text) = std::fs::read_to_string(&canon) {
                        seen.push(canon.clone());
                        let parent = canon.parent().unwrap_or(dir).to_path_buf();
                        out.push_str(&expand_imports(&text, &parent, depth + 1, seen));
                        out.push('\n');
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn push(files: &mut Vec<MemoryFile>, path: PathBuf, kind: MemoryKind) {
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    if text.trim().is_empty() || files.iter().any(|f| f.path == path) {
        return;
    }
    let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let mut seen = vec![path.canonicalize().unwrap_or(path.clone())];
    let content = expand_imports(&text, &dir, 0, &mut seen);
    files.push(MemoryFile { path, kind, content });
}

/// Memory for a session in `cwd`: user files, then project files from the
/// filesystem root down to `cwd` (closer files later, so they read last and
/// take precedence), then local files.
pub fn load_memory(cwd: &Path) -> Vec<MemoryFile> {
    let mut files = vec![];
    push(&mut files, claude_home().join("CLAUDE.md"), MemoryKind::User);
    push(&mut files, forge_home().join("FORGE.md"), MemoryKind::User);
    let home = home();
    let mut chain: Vec<&Path> = cwd.ancestors().take_while(|a| *a != home && a.parent().is_some()).collect();
    chain.reverse();
    for dir in chain {
        for name in ["CLAUDE.md", ".claude/CLAUDE.md", "FORGE.md", ".forge/FORGE.md"] {
            push(&mut files, dir.join(name), MemoryKind::Project);
        }
        for name in ["CLAUDE.local.md", "FORGE.local.md"] {
            push(&mut files, dir.join(name), MemoryKind::Local);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_imports() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let sub = root.join("app/pkg");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(root.join("app/CLAUDE.md"), "top rules\n@docs/extra.md\n```\n@not-imported\n```\n").unwrap();
        std::fs::create_dir_all(root.join("app/docs")).unwrap();
        std::fs::write(root.join("app/docs/extra.md"), "imported text").unwrap();
        std::fs::write(sub.join("FORGE.md"), "pkg rules").unwrap();
        std::fs::write(sub.join("FORGE.local.md"), "mine").unwrap();
        let files = load_memory(&sub);
        let project: Vec<_> = files.iter().filter(|f| f.path.starts_with(&root)).collect();
        assert_eq!(project.len(), 3);
        assert!(project[0].content.contains("imported text"));
        assert!(project[0].content.contains("@not-imported"));
        assert_eq!(project[1].content.trim(), "pkg rules");
        assert_eq!(project[2].kind, MemoryKind::Local);
    }

    #[test]
    fn import_cycles_stop() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        std::fs::write(root.join("CLAUDE.md"), "@a.md").unwrap();
        std::fs::write(root.join("a.md"), "A\n@CLAUDE.md").unwrap();
        let f = load_memory(&root);
        let mine = f.iter().find(|f| f.path.starts_with(&root)).unwrap();
        assert!(mine.content.contains('A'));
    }
}
