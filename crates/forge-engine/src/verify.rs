//! The verification loop (GOALS pillar 3).
//!
//! A turn that changed files may not end until the project's checks have run
//! since the last change. Otherwise the model gets one reminder (configurable)
//! naming the changed files and the detected check commands, and the loop goes
//! on. The final answer is asked to say what was verified and what was not.
//!
//! Evidence is a Bash call, made after the last change, that runs a check: a
//! configured or detected command, a well-known test/build/lint tool, a
//! `build*`/`test*` script, or one of the changed files itself.
//!
//! Changes are the edit tools' writes, from the file history, so sub-agent
//! edits count too. Shell commands that may write are confirmed against a git
//! worktree fingerprint, so a `sed -i` counts and an `ls` does not.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Settings for the verification loop (`verification` in settings).
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyConfig {
    /// The project's check commands: `verification.commands`, else detected.
    pub commands: Vec<String>,
    /// Reminders per user turn (`verification.maxReminders`, default 1).
    pub max_reminders: u32,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        VerifyConfig { commands: vec![], max_reminders: 1 }
    }
}

/// Check commands for the project at `dir`, read from its manifests.
pub fn detect_checks(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let has = |f: &str| dir.join(f).exists();
    let mut push = |c: &str| {
        if !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    };
    if has("Cargo.toml") {
        push("cargo build");
        push("cargo test");
    }
    if let Some(scripts) = std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("scripts").cloned())
    {
        let runner = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        for name in ["typecheck", "lint", "build", "test"] {
            let Some(body) = scripts.get(name).and_then(Value::as_str) else { continue };
            if name == "test" && body.contains("no test specified") {
                continue;
            }
            match (runner, name) {
                ("npm", "test") => push("npm test"),
                ("npm", n) => push(&format!("npm run {n}")),
                (r, n) => push(&format!("{r} {n}")),
            }
        }
    }
    if has("go.mod") {
        push("go build ./...");
        push("go test ./...");
    }
    let pyproject = std::fs::read_to_string(dir.join("pyproject.toml")).unwrap_or_default();
    if has("pytest.ini") || has("conftest.py") || pyproject.contains("[tool.pytest") || has("tox.ini") {
        push("python3 -m pytest");
    } else if has_python_tests(&dir.join("tests")) || has_python_tests(dir) {
        push("python3 -m unittest");
    }
    if let Ok(make) = std::fs::read_to_string(dir.join("Makefile")) {
        for target in ["check", "test"] {
            if make.lines().any(|l| l.starts_with(&format!("{target}:"))) {
                push(&format!("make {target}"));
            }
        }
    }
    for script in ["build.sh", "test.sh", "check.sh"] {
        if has(script) {
            push(&format!("./{script}"));
        }
    }
    if has("deno.json") || has("deno.jsonc") {
        push("deno test");
    }
    if has("pom.xml") {
        push("mvn -q test");
    }
    if has("build.gradle") || has("build.gradle.kts") {
        push(if has("gradlew") { "./gradlew test" } else { "gradle test" });
    }
    if has("Gemfile") && has("spec") {
        push("bundle exec rspec");
    }
    if has("mix.exs") {
        push("mix test");
    }
    if has("Package.swift") {
        push("swift test");
    }
    out
}

fn has_python_tests(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                (n.starts_with("test_") || n.ends_with("_test.py")) && n.ends_with(".py")
            })
        })
        .unwrap_or(false)
}

/// Well-known check tools, matched against a command's leading words.
const CHECK_PREFIXES: &[&str] = &[
    "cargo test",
    "cargo build",
    "cargo check",
    "cargo clippy",
    "cargo nextest",
    "cargo run",
    "npm test",
    "npm t",
    "npm run",
    "pnpm test",
    "pnpm run",
    "pnpm build",
    "pnpm lint",
    "pnpm typecheck",
    "yarn test",
    "yarn run",
    "yarn build",
    "yarn lint",
    "yarn typecheck",
    "bun test",
    "bun run",
    "npx tsc",
    "npx jest",
    "npx vitest",
    "npx eslint",
    "tsc",
    "jest",
    "vitest",
    "eslint",
    "node --test",
    "deno test",
    "deno check",
    "pytest",
    "py.test",
    "python -m pytest",
    "python3 -m pytest",
    "python -m unittest",
    "python3 -m unittest",
    "python -m mypy",
    "python3 -m mypy",
    "mypy",
    "ruff",
    "flake8",
    "pylint",
    "pyright",
    "tox",
    "nox",
    "python -m py_compile",
    "python3 -m py_compile",
    "python -m compileall",
    "python3 -m compileall",
    "go test",
    "go build",
    "go vet",
    "go run",
    "make",
    "cmake --build",
    "ctest",
    "ninja",
    "mvn",
    "gradle",
    "./gradlew",
    "dotnet test",
    "dotnet build",
    "bundle exec rspec",
    "rspec",
    "rake test",
    "bundle exec rake",
    "phpunit",
    "vendor/bin/phpunit",
    "composer test",
    "mix test",
    "swift test",
    "swift build",
    "zig build",
    "stack test",
    "cabal test",
    "dune test",
    "dune build",
    "gcc",
    "g++",
    "clang",
    "javac",
];

const INTERPRETERS: &[&str] =
    &["python", "python3", "node", "bash", "sh", "ruby", "perl", "php", "deno", "bun", "tsx", "ts-node"];

/// Words a command can start with that don't change what it runs.
fn skip_wrappers(words: &[String]) -> &[String] {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let assignment = w.contains('=') && !w.starts_with('-') && !w.starts_with('=');
        let step = if assignment || matches!(w, "env" | "time" | "nice" | "nohup" | "command" | "exec" | "xvfb-run") {
            1
        } else if w == "timeout"
            || (matches!(w, "uv" | "poetry" | "pipenv" | "hatch" | "pdm" | "rye")
                && words.get(i + 1).map(String::as_str) == Some("run"))
        {
            2
        } else {
            break;
        };
        i += step;
    }
    &words[i.min(words.len())..]
}

fn words(part: &str) -> Vec<String> {
    part.split_whitespace().map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string()).collect()
}

fn starts_with_words(words: &[String], prefix: &str) -> bool {
    let p: Vec<&str> = prefix.split_whitespace().collect();
    words.len() >= p.len() && words.iter().zip(&p).all(|(w, p)| w == p)
}

fn is_check_script(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase();
    ["build", "test", "check", "run_tests", "runtests", "verify", "ci"].iter().any(|p| name.starts_with(p))
        && (word.contains('/') || name.ends_with(".sh") || name.ends_with(".py"))
}

/// Does this Bash command run a check? `changed` are the files written this
/// turn, relative or absolute; running one of them counts as a check.
pub fn is_check(command: &str, configured: &[String], changed: &[PathBuf]) -> bool {
    forge_permissions::split_compound(command).iter().any(|part| {
        let all = words(part);
        let w = skip_wrappers(&all);
        let Some(first) = w.first() else { return false };
        if configured.iter().map(String::as_str).chain(CHECK_PREFIXES.iter().copied()).any(|c| starts_with_words(w, c))
        {
            return true;
        }
        if is_check_script(first) {
            return true;
        }
        let interp = INTERPRETERS.contains(&first.rsplit('/').next().unwrap_or(first));
        let args: Vec<&String> =
            if interp { w.iter().skip(1).filter(|a| !a.starts_with('-')).collect() } else { vec![] };
        if args.first().map(|a| is_check_script(a) || a.contains("test")).unwrap_or(false) {
            return true;
        }
        // Running a changed file: `python3 convert.py x.csv`, `./tool.sh`.
        let target = if interp { args.first().map(|a| a.as_str()) } else { Some(first.as_str()) };
        target
            .map(|t| {
                let t = t.trim_start_matches("./");
                !t.is_empty()
                    && changed.iter().any(|c| {
                        let c = c.to_string_lossy();
                        c == t || c.ends_with(&format!("/{t}"))
                    })
            })
            .unwrap_or(false)
    })
}

/// Per-turn verification state.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    /// File-history writes already covered by a check.
    pub write_mark: usize,
    /// A shell command that may have written ran since the last check.
    pub shell_may_have_changed: bool,
    /// Worktree fingerprint when the turn started or the last check ran.
    pub fingerprint: Option<u64>,
    /// The last check's command and whether it failed.
    pub last_check: Option<(String, bool)>,
    pub reminders: u32,
}

/// What the model is reminded of.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reminder {
    Unchecked { files: Vec<String>, shell: bool },
    LastCheckFailed { command: String },
}

impl Reminder {
    /// The transcript record (`system/verification`), for later analysis.
    pub fn record(&self) -> Value {
        match self {
            Reminder::Unchecked { files, shell } => {
                serde_json::json!({"kind": "unchecked", "files": files, "shell": shell})
            }
            Reminder::LastCheckFailed { command } => {
                serde_json::json!({"kind": "last_check_failed", "command": command})
            }
        }
    }

    pub fn text(&self, commands: &[String]) -> String {
        let how = if commands.is_empty() {
            "No check command was detected for this project: run the relevant tests, or run the changed code \
             directly"
                .to_string()
        } else {
            format!(
                "The project's checks: {}",
                commands.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ")
            )
        };
        match self {
            Reminder::Unchecked { files, shell } => {
                let what = match (files.is_empty(), shell) {
                    (false, _) => format!("You changed {} this turn", list(files)),
                    (true, _) => "Files in the working tree changed during this turn (through shell commands)".into(),
                };
                format!(
                    "<system-reminder>\n{what}, and no check has run since the last change.\n{how}.\nBefore you \
                     finish, run what applies and fix what fails. If a check can't run here, say why.\nIn your \
                     final answer, state what you verified and what you did not.\n</system-reminder>"
                )
            }
            Reminder::LastCheckFailed { command } => format!(
                "<system-reminder>\nThe last check (`{command}`) failed, and nothing changed after it.\nFix the \
                 failure, or, if it is expected or unrelated to your change, say so plainly in your final \
                 answer.\n</system-reminder>"
            ),
        }
    }
}

fn list(files: &[String]) -> String {
    const MAX: usize = 8;
    let mut s = files.iter().take(MAX).map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
    if files.len() > MAX {
        s.push_str(&format!(" and {} more", files.len() - MAX));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_checks_from_manifests() {
        let d = tempfile::tempdir().unwrap();
        assert!(detect_checks(d.path()).is_empty());
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"test":"vitest","lint":"eslint .","dev":"vite"}}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(d.path().join("test_stats.py"), "").unwrap();
        std::fs::write(d.path().join("Makefile"), "all:\n\ttrue\ntest:\n\ttrue\n").unwrap();
        std::fs::write(d.path().join("build.sh"), "").unwrap();
        assert_eq!(
            detect_checks(d.path()),
            ["cargo build", "cargo test", "pnpm lint", "pnpm test", "python3 -m unittest", "make test", "./build.sh"]
        );
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("package.json"), r#"{"scripts":{"test":"echo \"Error: no test specified\""}}"#)
            .unwrap();
        assert!(detect_checks(d.path()).is_empty(), "npm's placeholder test is not a check");
    }

    #[test]
    fn recognizes_check_commands() {
        let none: &[String] = &[];
        let changed = [PathBuf::from("/p/convert.py"), PathBuf::from("/p/src/lib.rs")];
        for yes in [
            "cargo test -q",
            "cd sub && cargo build --release",
            "python3 -m unittest -q test_stats",
            "PYTHONPATH=. pytest -x tests/",
            "uv run pytest",
            "timeout 60 npm test",
            "./build.sh",
            "bash scripts/test-all.sh",
            "python3 tests/test_api.py",
            "python3 convert.py sample.csv",
            "go vet ./... 2>&1 | head",
            "make",
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
        for no in ["ls -la", "cat convert.py", "git status", "echo test", "sed -i s/a/b/ x.py", "python3 -c 'print(1)'"]
        {
            assert!(!is_check(no, none, &changed), "{no}");
        }
        assert!(is_check("just verify", &["just verify".into()], &changed), "configured commands count");
    }

    #[test]
    fn reminder_text_names_files_and_checks() {
        let r = Reminder::Unchecked { files: vec!["a.py".into()], shell: false };
        let t = r.text(&["python3 -m unittest".into()]);
        assert!(t.contains("`a.py`") && t.contains("`python3 -m unittest`") && t.contains("what you did not"), "{t}");
        let t = Reminder::Unchecked { files: vec![], shell: true }.text(&[]);
        assert!(t.contains("shell commands") && t.contains("No check command was detected"), "{t}");
        let many: Vec<String> = (0..10).map(|i| format!("f{i}")).collect();
        assert!(Reminder::Unchecked { files: many, shell: false }.text(&[]).contains("and 2 more"));
    }
}
