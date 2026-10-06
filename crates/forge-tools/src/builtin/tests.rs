use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{Tool, ToolContext};

fn ctx(dir: &std::path::Path) -> ToolContext {
    let dir = dir.canonicalize().unwrap();
    ToolContext::new(&dir)
}

fn p(dir: &std::path::Path, name: &str) -> String {
    dir.canonicalize().unwrap().join(name).display().to_string()
}

#[tokio::test]
async fn bash_runs_and_keeps_cwd_inside_project() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    let c = ctx(d.path());
    let out = Bash.call(json!({"command": "cd sub && echo hi"}), &c).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.text_content(), "hi");
    let out = Bash.call(json!({"command": "pwd"}), &c).await;
    assert!(out.text_content().ends_with("/sub"), "{}", out.text_content());
    // Leaving the working directories resets the cwd.
    let out = Bash.call(json!({"command": "cd / && true"}), &c).await;
    assert!(out.text_content().contains("Shell cwd was reset"), "{}", out.text_content());
    assert_eq!(c.shell_cwd(), c.project_dir);
}

#[tokio::test]
async fn bash_reports_exit_code_and_stderr() {
    let d = tempfile::tempdir().unwrap();
    let out = Bash.call(json!({"command": "echo out; echo err >&2; exit 3"}), &ctx(d.path())).await;
    assert!(out.is_error);
    let t = out.text_content();
    assert!(t.starts_with("Exit code 3"), "{t}");
    assert!(t.contains("out") && t.contains("err"));
}

#[tokio::test]
async fn bash_timeout_kills_process_group() {
    let d = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let out = Bash.call(json!({"command": "sleep 30 & sleep 30", "timeout": 300}), &ctx(d.path())).await;
    assert!(out.is_error);
    assert!(out.text_content().contains("timed out"));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn bash_interrupt_returns_interrupted() {
    let d = tempfile::tempdir().unwrap();
    let base = ctx(d.path());
    let cancel = CancellationToken::new();
    let c = base.for_call("t1", cancel.clone());
    let task = tokio::spawn(async move { Bash.call(json!({"command": "sleep 20"}), &c).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();
    let out = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    assert!(out.is_error);
    assert!(out.text_content().contains(crate::INTERRUPTED));
}

#[tokio::test]
async fn background_shell_output_and_kill() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path());
    let out = Bash.call(json!({"command": "echo started; sleep 20", "run_in_background": true}), &c).await;
    assert!(out.text_content().contains("bash_1"), "{}", out.text_content());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let o = BashOutput.call(json!({"bash_id": "bash_1"}), &c).await;
    assert!(
        o.text_content().contains("started") && o.text_content().contains("<status>running"),
        "{}",
        o.text_content()
    );
    let o = BashOutput.call(json!({"bash_id": "bash_1"}), &c).await;
    assert!(!o.text_content().contains("started"), "output is only returned once");
    let k = KillShell.call(json!({"shell_id": "bash_1"}), &c).await;
    assert!(!k.is_error);
    assert_eq!(c.shells.get("bash_1").unwrap().status(), crate::shells::ShellStatus::Killed);
}

#[test]
fn read_only_classification() {
    assert!(bash::command_is_read_only("ls -la && git status"));
    assert!(bash::command_is_read_only("grep -r foo . | head"));
    assert!(!bash::command_is_read_only("echo hi > out.txt"));
    assert!(!bash::command_is_read_only("git push"));
    assert!(!bash::command_is_read_only("find . -delete"));
    assert!(!bash::command_is_read_only("cat $(rm x)"));
}

#[tokio::test]
async fn read_numbers_lines_and_pages() {
    let d = tempfile::tempdir().unwrap();
    let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    std::fs::write(d.path().join("a.txt"), body).unwrap();
    let c = ctx(d.path());
    let out = Read.call(json!({"file_path": p(d.path(), "a.txt"), "offset": 3, "limit": 2}), &c).await;
    assert_eq!(out.text_content(), "     3\tline 3\n     4\tline 4\n");
    let out = Read.call(json!({"file_path": p(d.path(), "missing.txt")}), &c).await;
    assert!(out.is_error);
    let out = Read.call(json!({"file_path": c.project_dir.display().to_string()}), &c).await;
    assert!(out.is_error && out.text_content().contains("directory"));
}

#[tokio::test]
async fn edit_requires_read_and_unique_match() {
    let d = tempfile::tempdir().unwrap();
    let f = p(d.path(), "f.rs");
    std::fs::write(&f, "a = 1;\nb = 1;\n").unwrap();
    let c = ctx(d.path());
    let input = json!({"file_path": f, "old_string": "= 1", "new_string": "= 2"});
    assert!(Edit.validate(&input, &c).unwrap_err().contains("has not been read"));
    Read.call(json!({"file_path": f}), &c).await;
    Edit.validate(&input, &c).unwrap();
    let out = Edit.call(input.clone(), &c).await;
    assert!(out.is_error && out.text_content().contains("Found 2 matches"));
    let out = Edit.call(json!({"file_path": f, "old_string": "a = 1", "new_string": "a = 2"}), &c).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "a = 2;\nb = 1;\n");
    // Our own write keeps the file editable.
    Edit.validate(&json!({"file_path": f, "old_string": "b = 1", "new_string": "b = 3"}), &c).unwrap();
    // An outside modification makes it stale.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&f, "changed\n").unwrap();
    filetime_bump(&f);
    assert!(Edit
        .validate(&json!({"file_path": f, "old_string": "changed", "new_string": "x"}), &c)
        .unwrap_err()
        .contains("modified since read"));
}

/// Make sure the mtime differs even on coarse-grained filesystems.
fn filetime_bump(f: &str) {
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    let file = std::fs::OpenOptions::new().write(true).open(f).unwrap();
    file.set_modified(later).unwrap();
}

#[tokio::test]
async fn edit_preserves_crlf_and_multiedit_is_atomic() {
    let d = tempfile::tempdir().unwrap();
    let f = p(d.path(), "w.txt");
    std::fs::write(&f, "one\r\ntwo\r\n").unwrap();
    let c = ctx(d.path());
    Read.call(json!({"file_path": f}), &c).await;
    let out = Edit.call(json!({"file_path": f, "old_string": "one\ntwo", "new_string": "1\n2"}), &c).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "1\r\n2\r\n");

    std::fs::write(&f, "x y z").unwrap();
    Read.call(json!({"file_path": f}), &c).await;
    let out = MultiEdit
        .call(json!({"file_path": f, "edits": [{"old_string": "x", "new_string": "X"}, {"old_string": "nope", "new_string": "?"}]}), &c)
        .await;
    assert!(out.is_error && out.text_content().contains("Edit 2 of 2"));
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "x y z", "nothing written on failure");
}

#[tokio::test]
async fn write_creates_and_checkpoints() {
    struct Rec(std::sync::Mutex<Vec<std::path::PathBuf>>);
    impl crate::Checkpointer for Rec {
        fn before_write(&self, p: &std::path::Path) {
            self.0.lock().unwrap().push(p.to_path_buf());
        }
    }
    let d = tempfile::tempdir().unwrap();
    let rec = Arc::new(Rec(Default::default()));
    let mut c = ctx(d.path());
    c.checkpointer = Some(rec.clone());
    let f = p(d.path(), "new/dir/file.txt");
    let out = Write.call(json!({"file_path": f, "content": "hello"}), &c).await;
    assert!(out.text_content().contains("created"), "{out:?}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "hello");
    assert_eq!(rec.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn glob_and_grep() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("src/inner")).unwrap();
    std::fs::write(d.path().join("src/a.rs"), "fn main() {}\n// TODO one\n").unwrap();
    std::fs::write(d.path().join("src/inner/b.rs"), "struct B;\n// todo two\nx\ny\n").unwrap();
    std::fs::write(d.path().join("notes.md"), "TODO three\n").unwrap();
    std::fs::write(d.path().join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir(d.path().join("ignored")).unwrap();
    std::fs::write(d.path().join("ignored/c.rs"), "TODO hidden").unwrap();
    std::process::Command::new("git").arg("init").arg("-q").current_dir(d.path()).status().unwrap();
    let c = ctx(d.path());

    let g = Glob.call(json!({"pattern": "**/*.rs"}), &c).await;
    let t = g.text_content();
    assert!(t.contains("a.rs") && t.contains("b.rs") && !t.contains("c.rs"), "{t}");
    assert_eq!(Glob.call(json!({"pattern": "*.py"}), &c).await.text_content(), "No files found");

    let files = Grep.call(json!({"pattern": "TODO"}), &c).await.text_content();
    assert!(files.starts_with("Found 2 files"), "{files}");
    let ci = Grep
        .call(json!({"pattern": "todo", "-i": true, "glob": "*.rs", "output_mode": "count"}), &c)
        .await
        .text_content();
    assert!(ci.contains("a.rs:1") && ci.contains("b.rs:1"), "{ci}");
    let content = Grep.call(json!({"pattern": "todo two", "output_mode": "content", "-A": 1}), &c).await.text_content();
    assert!(content.contains("b.rs:2:// todo two") && content.contains("b.rs-3-x"), "{content}");
    let typed = Grep.call(json!({"pattern": "TODO", "type": "md"}), &c).await.text_content();
    assert!(typed.contains("notes.md") && !typed.contains("a.rs"), "{typed}");
    let ml = Grep.call(json!({"pattern": "struct B;.*two", "multiline": true}), &c).await.text_content();
    assert!(ml.contains("b.rs"), "{ml}");
}

#[tokio::test]
async fn ls_lists_tree() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("a/b")).unwrap();
    std::fs::write(d.path().join("a/b/f.txt"), "").unwrap();
    std::fs::write(d.path().join(".hidden"), "").unwrap();
    let c = ctx(d.path());
    let t = Ls.call(json!({"path": c.project_dir.display().to_string()}), &c).await.text_content();
    assert!(t.contains("  - a/\n    - b/\n      - f.txt"), "{t}");
    assert!(!t.contains(".hidden"));
}

#[tokio::test]
async fn todo_write_validates_and_stores() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path());
    assert!(TodoWrite.validate(&json!({"todos": [{"content": "x", "status": "bad", "activeForm": "x"}]}), &c).is_err());
    let input = json!({"todos": [{"content": "Run tests", "status": "in_progress", "activeForm": "Running tests"}]});
    TodoWrite.validate(&input, &c).unwrap();
    TodoWrite.call(input, &c).await;
    assert_eq!(c.todos.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn notebook_edit_replace_insert_delete() {
    let d = tempfile::tempdir().unwrap();
    let f = p(d.path(), "n.ipynb");
    std::fs::write(&f, r#"{"cells":[{"cell_type":"code","id":"c1","metadata":{},"source":["print(1)"],"outputs":[{"text":["1\n"]}],"execution_count":1}],"metadata":{},"nbformat":4,"nbformat_minor":5}"#).unwrap();
    let c = ctx(d.path());
    let r = Read.call(json!({"file_path": f}), &c).await.text_content();
    assert!(r.contains("<cell id=\"c1\"") && r.contains("print(1)") && r.contains("<output cell=\"c1\">"), "{r}");
    let out = NotebookEdit.call(json!({"notebook_path": f, "cell_id": "c1", "new_source": "print(2)"}), &c).await;
    assert!(!out.is_error, "{out:?}");
    NotebookEdit.call(json!({"notebook_path": f, "cell_id": "c1", "new_source": "# Title", "cell_type": "markdown", "edit_mode": "insert"}), &c).await;
    let nb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(nb["cells"].as_array().unwrap().len(), 2);
    assert_eq!(nb["cells"][0]["source"][0], "print(2)");
    NotebookEdit.call(json!({"notebook_path": f, "cell_id": "c1", "new_source": "", "edit_mode": "delete"}), &c).await;
    let nb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(nb["cells"].as_array().unwrap().len(), 1);
}

#[test]
fn apply_edit_rules() {
    assert!(apply_edit("a", "a", "a", false).is_err());
    assert_eq!(apply_edit("", "", "new", false).unwrap(), "new");
    assert_eq!(apply_edit("aa", "a", "b", true).unwrap(), "bb");
    assert!(apply_edit("abc", "x", "y", false).unwrap_err().contains("not found"));
}

fn sandboxed_ctx(dir: &std::path::Path) -> Option<ToolContext> {
    crate::sandbox::backend()?;
    let mut c = ctx(dir);
    c.sandbox = Some(Arc::new(crate::sandbox::SandboxPolicy {
        mode: crate::sandbox::SandboxMode::WorkspaceWrite,
        network: false,
        writable_roots: vec![],
        extra_writable: vec![],
    }));
    Some(c)
}

#[tokio::test]
async fn sandbox_confines_writes_and_network() {
    // The system temp dirs are writable inside the sandbox, so the test tree lives under target/.
    let root = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target")).unwrap();
    let proj = root.path().join("proj");
    std::fs::create_dir_all(proj.join(".forge")).unwrap();
    std::fs::write(proj.join(".forge/settings.json"), "{}").unwrap();
    let Some(c) = sandboxed_ctx(&proj) else {
        eprintln!("skipped: no sandbox backend on this machine");
        return;
    };
    let input = json!({"command": "echo inside > made.txt && cat made.txt"});
    assert!(Bash.sandboxed(&input, &c));
    let out = Bash.call(input, &c).await;
    assert!(!out.is_error, "{out:?}");
    assert!(c.project_dir.join("made.txt").exists());

    let outside = root.path().join("outside.txt");
    let out = Bash.call(json!({"command": format!("touch '{}'", outside.display())}), &c).await;
    assert!(out.is_error && !outside.exists(), "writes outside the workspace fail: {out:?}");
    assert!(
        out.text_content().contains("dangerouslyDisableSandbox"),
        "the failure names the way out: {}",
        out.text_content()
    );

    let out = Bash
        .call(json!({"command": "echo '{\"permissions\":{\"allow\":[\"Bash\"]}}' > .forge/settings.json"}), &c)
        .await;
    assert!(out.is_error, "settings stay read-only inside the sandbox");
    assert_eq!(std::fs::read_to_string(c.project_dir.join(".forge/settings.json")).unwrap(), "{}");

    let out = Bash.call(json!({"command": "cat < /dev/tcp/1.1.1.1/53 || exit 7"}), &c).await;
    assert!(out.is_error, "no network in the sandbox");

    // Escalation runs unconfined (permission is the engine's job).
    let esc = json!({"command": format!("touch '{}'", outside.display()), "dangerouslyDisableSandbox": true});
    assert!(!Bash.sandboxed(&esc, &c));
    assert!(!Bash.call(esc, &c).await.is_error);
    assert!(outside.exists());
}
