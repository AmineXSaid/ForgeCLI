//! Paths as people and models should see them.

use std::path::{Path, PathBuf};

/// `\\?\C:\x` -> `C:\x` and `\\?\UNC\srv\share\x` -> `\\srv\share\x`, when the
/// short form names the same file. `None` when the prefix must stay (a
/// reserved device name, a trailing dot or space, a character the short form
/// rejects, or a path too long for it).
pub fn simplify_verbatim(s: &str) -> Option<String> {
    let rest = s.strip_prefix(r"\\?\")?;
    let (out, tail) = if let Some(unc) = rest.strip_prefix(r"UNC\") {
        (format!(r"\\{unc}"), unc)
    } else {
        let b = rest.as_bytes();
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        if !drive || (b.len() > 2 && b[2] != b'\\') {
            return None; // \\?\Volume{...}, \\?\GLOBALROOT, ...
        }
        if b.len() == 2 {
            return Some(format!(r"{rest}\"));
        }
        (rest.to_string(), &rest[3..])
    };
    if out.len() >= 260 {
        return None;
    }
    for c in tail.split('\\').filter(|c| !c.is_empty()) {
        if c == "." || c == ".." || c.ends_with('.') || c.ends_with(' ') {
            return None;
        }
        if c.chars().any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*') || (ch as u32) < 32) {
            return None;
        }
        let stem = c.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if device {
            return None;
        }
    }
    Some(out)
}

/// The path without the verbatim prefix `canonicalize` adds on Windows; unchanged elsewhere.
pub fn simplify(p: &Path) -> PathBuf {
    if cfg!(windows) {
        if let Some(s) = p.to_str().and_then(simplify_verbatim) {
            return PathBuf::from(s);
        }
    }
    p.to_path_buf()
}

/// `std::fs::canonicalize`, simplified: use it everywhere a path is shown,
/// sent to the model or compared with paths the model wrote.
pub fn canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(p).map(|c| simplify(&c))
}

/// An MSYS / Cygwin path (`/c/Users/me`, `/cygdrive/c/Users/me`) or a mixed one
/// (`C:/Users/me`) in Windows form (`C:\Users\me`). `None` for a path that only
/// exists inside the MSYS tree (`/usr/bin`, `/tmp`).
pub fn msys_to_windows(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Some(s.replace('/', "\\"));
    }
    let rest = s.strip_prefix("/cygdrive/").map(|r| format!("/{r}"));
    let s = rest.as_deref().unwrap_or(s);
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/') {
        let drive = (b[1] as char).to_ascii_uppercase();
        let tail = if b.len() > 3 { s[3..].replace('/', "\\") } else { String::new() };
        return Some(format!("{drive}:\\{tail}"));
    }
    None
}

/// A path as the OS takes it: on Windows an MSYS or mixed path (`/c/x`,
/// `C:/x`) in native form; unchanged elsewhere.
pub fn native(s: &str) -> PathBuf {
    if cfg!(windows) {
        if let Some(w) = msys_to_windows(s) {
            return PathBuf::from(w);
        }
    }
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefix_is_dropped_only_when_safe() {
        assert_eq!(simplify_verbatim(r"\\?\C:\Users\me\proj").as_deref(), Some(r"C:\Users\me\proj"));
        assert_eq!(simplify_verbatim(r"\\?\C:").as_deref(), Some(r"C:\"));
        assert_eq!(simplify_verbatim(r"\\?\UNC\srv\share\dir").as_deref(), Some(r"\\srv\share\dir"));
        assert_eq!(simplify_verbatim(r"\\?\C:\a\CON\b"), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\a\com1.txt"), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\a\b."), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\a\b "), None);
        assert_eq!(simplify_verbatim(r"\\?\Volume{0b1c}\x"), None);
        assert_eq!(simplify_verbatim(&format!(r"\\?\C:\{}", "a".repeat(300))), None);
        assert_eq!(simplify_verbatim(r"C:\already\short"), None);
        assert_eq!(simplify_verbatim("/home/me"), None);
        assert_eq!(simplify(Path::new("/home/me")), PathBuf::from("/home/me"));
    }

    #[test]
    fn msys_paths() {
        assert_eq!(msys_to_windows("/c/Users/me").as_deref(), Some(r"C:\Users\me"));
        assert_eq!(msys_to_windows("/d").as_deref(), Some(r"D:\"));
        assert_eq!(msys_to_windows("/cygdrive/e/x/y").as_deref(), Some(r"E:\x\y"));
        assert_eq!(msys_to_windows("C:/Users/me").as_deref(), Some(r"C:\Users\me"));
        assert_eq!(msys_to_windows("/usr/bin"), None);
        assert_eq!(msys_to_windows("/tmp"), None);
    }
}
