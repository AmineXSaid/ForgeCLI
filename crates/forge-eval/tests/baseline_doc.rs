//! docs/BASELINE.md, "Recording the first real baseline": every `forge-eval`
//! command it gives must parse, so a renamed flag breaks the build rather than
//! the first real run.

use std::path::Path;
use std::process::Command;

/// The `forge-eval ...` command lines in the section, as argument lists (program name dropped).
fn commands() -> Vec<Vec<String>> {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/BASELINE.md")).unwrap();
    let section = doc.split("## Recording the first real baseline").nth(1).expect("the section exists");
    section
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("target/release/forge-eval "))
        .map(|l| shlex::split(l).expect("shell words")[1..].to_vec())
        .collect()
}

#[test]
fn baseline_how_to_commands_parse() {
    let cmds = commands();
    let subs: Vec<&str> = cmds.iter().map(|c| c[0].as_str()).collect();
    for want in ["validate", "list", "run", "compare"] {
        assert!(subs.contains(&want), "the how-to uses forge-eval {want}: {subs:?}");
    }
    for mut args in cmds {
        // `--help` goes before `--`: what follows is passed to forge, not parsed here.
        let at = args.iter().position(|a| a == "--").unwrap_or(args.len());
        args.insert(at, "--help".into());
        let out = Command::new(env!("CARGO_BIN_EXE_forge-eval")).args(&args).output().unwrap();
        assert!(
            out.status.success(),
            "forge-eval {} failed to parse:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn baseline_paths_match_the_code() {
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/BASELINE.md")).unwrap();
    let help = Command::new(env!("CARGO_BIN_EXE_forge-eval")).args(["run", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("evals/results/<label>") && doc.contains("evals/results/baseline/"));
    assert!(help.contains("[default: target/release/forge]") && doc.contains("default `target/release/forge`"));
    assert!(help.contains("[default: 2]") && doc.contains("(default 2)"));
    let gitignore = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l == "/evals/results"), "results stay out of git");
}
