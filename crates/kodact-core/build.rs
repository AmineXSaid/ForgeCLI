//! `/release-notes` shows CHANGELOG.md. The crate's own copy is a symlink to the
//! repository's root file, and a real file in the published crate. A checkout
//! without symlinks (Git for Windows' default) leaves only the link's target
//! text there, so then the root file is read directly.

use std::path::Path;

fn main() {
    let dir = Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR")).to_path_buf();
    let own = dir.join("CHANGELOG.md");
    let is_changelog = |p: &Path| std::fs::read_to_string(p).is_ok_and(|s| s.starts_with("# Changelog"));
    let changelog = if is_changelog(&own) { own } else { dir.join("../../CHANGELOG.md") };
    println!("cargo:rerun-if-changed={}", changelog.display());
    println!("cargo:rustc-env=KODACT_CHANGELOG={}", changelog.display());
}
