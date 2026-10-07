//! `@path` mentions: the files and directories a message names, read and
//! attached so the model sees them without a Read call.
//!
//! Used by custom commands (their body's `@path`) and by every plain prompt
//! (`forge_core::Driver::input`).
//!
//! - `@path` counts at the start of the text or after whitespace, so emails
//!   (`me@example.com`) don't. Trailing `.,:;)!?` is punctuation, not path.
//! - `@"path with spaces.md"` quotes a path.
//! - `~/` is the home directory; other relative paths start at `cwd`.
//! - A mention with no such file or directory is left alone, with no note.
//! - Each path is attached once.
//!
//! Limits ([`MAX_FILE_CHARS`], [`MAX_FILES`], [`MAX_TOTAL_CHARS`],
//! [`MAX_DIR_ENTRIES`]) keep one prompt from filling the context. What they
//! leave out gets a one-line note telling the model to use Read.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// Characters of one file's text (the middle is cut past this).
pub const MAX_FILE_CHARS: usize = 50_000;
/// Files and directories attached from one message.
pub const MAX_FILES: usize = 10;
/// Characters attached from one message, all files together.
pub const MAX_TOTAL_CHARS: usize = 200_000;
/// Entries in a directory listing.
pub const MAX_DIR_ENTRIES: usize = 200;
/// Files larger than this aren't read at all (the text would be cut anyway).
const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    pub kind: Kind,
    /// The file's text or the directory's listing; empty when skipped.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    File,
    Directory,
    /// Mentioned and present, but not attached: why, for the model.
    Skipped(String),
}

static AT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:^|\s)@(?:"([^"\n]+)"|([^\s"]+))"#).expect("regex"));

/// The paths `text` mentions, as written (quotes removed), in order, each once.
pub fn mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for c in AT.captures_iter(text) {
        let raw = match (c.get(1), c.get(2)) {
            (Some(q), _) => q.as_str(),
            (None, Some(u)) => u.as_str().trim_end_matches(['.', ',', ':', ';', ')', '!', '?']),
            _ => continue,
        };
        if !raw.is_empty() && !out.iter().any(|o| o == raw) {
            out.push(raw.to_string());
        }
    }
    out
}

fn resolve(raw: &str, cwd: &Path) -> PathBuf {
    let p = if let Some(rest) = raw.strip_prefix("~/") {
        forge_config::home().join(rest)
    } else if raw == "~" {
        forge_config::home()
    } else {
        cwd.join(raw)
    };
    forge_permissions::normalize(&p, cwd)
}

fn media_kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svgz" | "tiff" => "an image",
        "pdf" => "a PDF",
        _ => return None,
    })
}

/// A short, ignore-aware listing: paths relative to `dir`, directories ending in `/`.
fn listing(dir: &Path) -> String {
    let mut entries = vec![];
    let mut more = false;
    for e in ignore::WalkBuilder::new(dir).sort_by_file_name(|a, b| a.cmp(b)).build().flatten() {
        let Ok(rel) = e.path().strip_prefix(dir) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        if entries.len() >= MAX_DIR_ENTRIES {
            more = true;
            break;
        }
        let mut p = rel.to_string_lossy().replace('\\', "/");
        if e.file_type().is_some_and(|t| t.is_dir()) {
            p.push('/');
        }
        entries.push(p);
    }
    if entries.is_empty() {
        return "(empty)".into();
    }
    let mut s = entries.join("\n");
    if more {
        s.push_str(&format!("\n(listing stops at {MAX_DIR_ENTRIES} entries)"));
    }
    s
}

/// The text of a file worth attaching, or why it isn't.
fn file_text(path: &Path) -> Result<String, String> {
    if let Some(what) = media_kind(path) {
        return Err(format!("{what}; use Read to view it"));
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_READ_BYTES {
        return Err(format!("{size} bytes, too large to attach; use Read with offset and limit"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("could not read it ({e}); use Read"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err("a binary file; use Read to view it".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "a binary file; use Read to view it".to_string())?;
    Ok(forge_tools::truncate_middle(text.trim_end(), MAX_FILE_CHARS))
}

/// What `text` mentions, read. `allowed` says whether a path may be read
/// without asking (`Err` holds the reason it may not); mentions with no such
/// file or directory are dropped.
pub fn at_mentions(text: &str, cwd: &Path, allowed: &dyn Fn(&Path) -> Result<(), String>) -> Vec<Attachment> {
    let mut out: Vec<Attachment> = vec![];
    let mut attached = 0;
    let mut total = 0;
    for raw in mentions(text) {
        let path = resolve(&raw, cwd);
        if !path.exists() || out.iter().any(|a| a.path == path) {
            continue;
        }
        let skip = |path: PathBuf, why: String| Attachment { path, kind: Kind::Skipped(why), text: String::new() };
        if let Err(why) = allowed(&path) {
            out.push(skip(path, format!("{why}; use Read")));
            continue;
        }
        if attached >= MAX_FILES {
            out.push(skip(path, format!("one message attaches at most {MAX_FILES} files; use Read")));
            continue;
        }
        let (kind, body) = if path.is_dir() {
            (Kind::Directory, listing(&path))
        } else {
            match file_text(&path) {
                Ok(t) => (Kind::File, t),
                Err(why) => {
                    out.push(skip(path, why));
                    continue;
                }
            }
        };
        let chars = body.chars().count();
        if total + chars > MAX_TOTAL_CHARS {
            out.push(skip(path, format!("one message attaches at most {MAX_TOTAL_CHARS} characters; use Read")));
            continue;
        }
        total += chars;
        attached += 1;
        out.push(Attachment { path, kind, text: body });
    }
    out
}

/// The attachments as `<file>` and `<directory>` blocks and one-line notes,
/// separated by blank lines.
pub fn render(atts: &[Attachment]) -> String {
    atts.iter()
        .map(|a| {
            let p = a.path.display();
            match &a.kind {
                Kind::File => format!("<file path=\"{p}\">\n{}\n</file>", a.text),
                Kind::Directory => format!("<directory path=\"{p}\">\n{}\n</directory>", a.text),
                Kind::Skipped(why) => format!("{p} was not attached: {why}."),
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The system reminder a prompt's attachments travel in, or `None` when there are none.
pub fn reminder(atts: &[Attachment]) -> Option<String> {
    (!atts.is_empty()).then(|| {
        format!(
            "<system-reminder>\nThe user's message mentions these paths with @. Their contents as of now (no \
             Read needed for these):\n\n{}\n</system-reminder>",
            render(atts)
        )
    })
}

static ATTACHED_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^<(?:file|directory) path="([^"]+)">$"#).expect("regex"));

/// The paths a [`reminder`] attached (for front ends that show them).
pub fn attached_paths(reminder: &str) -> Vec<String> {
    if !reminder.starts_with("<system-reminder>\nThe user's message mentions these paths with @.") {
        return vec![];
    }
    ATTACHED_PATH.captures_iter(reminder).map(|c| c[1].to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(_: &Path) -> Result<(), String> {
        Ok(())
    }

    fn dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(d.path().join("my notes.md"), "spaced").unwrap();
        d
    }

    #[test]
    fn matches_start_whitespace_and_trims_punctuation() {
        assert_eq!(mentions("@a.rs, then (see @b.rs). Done @c.rs!"), ["a.rs", "b.rs", "c.rs"]);
        assert_eq!(mentions("what is @d.rs?\n@e/f.txt:"), ["d.rs", "e/f.txt"]);
        assert_eq!(mentions("mail me@example.com or x@y"), Vec::<String>::new());
    }

    #[test]
    fn quoted_paths_and_dedup() {
        let d = dir();
        assert_eq!(mentions(r#"read @"my notes.md" and @a.rs and @a.rs"#), ["my notes.md", "a.rs"]);
        let atts = at_mentions(r#"@"my notes.md" @a.rs @./a.rs"#, d.path(), &ok);
        assert_eq!(atts.len(), 2, "{atts:?}");
        assert_eq!((atts[0].kind.clone(), atts[0].text.as_str()), (Kind::File, "spaced"));
        assert_eq!(atts[1].path, d.path().join("a.rs"));
    }

    #[test]
    fn missing_paths_and_emails_are_left_alone() {
        let d = dir();
        std::fs::write(d.path().join("example.com"), "not me").unwrap();
        assert!(at_mentions("@alice said hi to me@example.com about @missing.rs", d.path(), &ok).is_empty());
    }

    #[test]
    fn home_relative_paths() {
        let home = forge_config::home();
        assert_eq!(resolve("~/x.md", Path::new("/w")), forge_permissions::normalize(&home.join("x.md"), &home));
        assert_eq!(resolve("src/../a.rs", Path::new("/w")), PathBuf::from("/w/a.rs"));
    }

    #[test]
    fn directories_are_listed_ignore_aware() {
        let d = dir();
        std::fs::create_dir_all(d.path().join("src/inner")).unwrap();
        std::fs::create_dir_all(d.path().join("src/target")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        std::fs::write(d.path().join("src/target/out"), "").unwrap();
        std::fs::write(d.path().join("src/.ignore"), "target/\n").unwrap();
        let atts = at_mentions("look at @src/", d.path(), &ok);
        assert_eq!(atts[0].kind, Kind::Directory);
        assert_eq!(atts[0].text, "inner/\nmain.rs");
        let big = d.path().join("big");
        std::fs::create_dir(&big).unwrap();
        for i in 0..MAX_DIR_ENTRIES + 5 {
            std::fs::write(big.join(format!("f{i:03}")), "").unwrap();
        }
        let atts = at_mentions("@big", d.path(), &ok);
        assert_eq!(atts[0].text.lines().count(), MAX_DIR_ENTRIES + 1);
        assert!(atts[0].text.ends_with("(listing stops at 200 entries)"));
    }

    #[test]
    fn binary_images_and_pdfs_get_a_note() {
        let d = dir();
        std::fs::write(d.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(d.path().join("pic.png"), "png").unwrap();
        std::fs::write(d.path().join("doc.pdf"), "%PDF").unwrap();
        let atts = at_mentions("@blob.bin @pic.png @doc.pdf", d.path(), &ok);
        let notes: Vec<_> = atts.iter().map(|a| a.kind.clone()).collect();
        assert_eq!(
            notes,
            [
                Kind::Skipped("a binary file; use Read to view it".into()),
                Kind::Skipped("an image; use Read to view it".into()),
                Kind::Skipped("a PDF; use Read to view it".into()),
            ]
        );
        assert!(render(&atts).starts_with(&format!(
            "{} was not attached: a binary file; use Read to view it.",
            d.path().join("blob.bin").display()
        )));
    }

    #[test]
    fn permission_refusals_become_notes() {
        let d = dir();
        let deny = |p: &Path| if p.ends_with("a.rs") { Err("blocked by a permission rule".into()) } else { Ok(()) };
        let atts = at_mentions("@a.rs @\"my notes.md\"", d.path(), &deny);
        assert_eq!(atts[0].kind, Kind::Skipped("blocked by a permission rule; use Read".into()));
        assert!(atts[0].text.is_empty());
        assert_eq!(atts[1].kind, Kind::File);
    }

    #[test]
    fn limits_per_file_count_and_total() {
        let d = dir();
        std::fs::write(d.path().join("long.txt"), "x".repeat(MAX_FILE_CHARS * 2)).unwrap();
        let atts = at_mentions("@long.txt", d.path(), &ok);
        assert!(atts[0].text.chars().count() < MAX_FILE_CHARS + 200, "cut to about the limit");

        let mut text = String::new();
        for i in 0..MAX_FILES + 2 {
            std::fs::write(d.path().join(format!("f{i}.txt")), "small").unwrap();
            text.push_str(&format!("@f{i}.txt "));
        }
        let atts = at_mentions(&text, d.path(), &ok);
        assert_eq!(atts.iter().filter(|a| a.kind == Kind::File).count(), MAX_FILES);
        assert!(matches!(&atts[MAX_FILES].kind, Kind::Skipped(w) if w.contains("at most 10 files")));

        let mut text = String::new();
        for i in 0..5 {
            std::fs::write(d.path().join(format!("big{i}.txt")), "y".repeat(MAX_FILE_CHARS - 1000)).unwrap();
            text.push_str(&format!("@big{i}.txt "));
        }
        let atts = at_mentions(&text, d.path(), &ok);
        assert_eq!(atts.iter().filter(|a| a.kind == Kind::File).count(), 4);
        assert!(matches!(&atts[4].kind, Kind::Skipped(w) if w.contains("200000 characters")));
    }

    #[test]
    fn reminder_names_its_paths() {
        let d = dir();
        let atts = at_mentions("@a.rs", d.path(), &ok);
        let r = reminder(&atts).unwrap();
        assert!(r.starts_with("<system-reminder>\n") && r.ends_with("</system-reminder>"));
        assert_eq!(attached_paths(&r), [d.path().join("a.rs").display().to_string()]);
        assert!(reminder(&[]).is_none());
        assert!(attached_paths("<file path=\"x\">\n</file>").is_empty());
    }
}
