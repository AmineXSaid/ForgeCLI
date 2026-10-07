You are continuing work on **ForgeCLI**. It is a Rust-native agentic coding
CLI (repo `AmineXSaid/ForgeCLI`) whose behavioural reference is the leading
commercial terminal coding agent. This is prompt **1 of 4** in `docs/next/`.
Work autonomously to the end of the list below. Don't ask me questions; make
reasonable decisions and record them in the docs.

## Branch

Work and push only on **`claude/youthful-mayer-qs7e0h`**:
`git fetch origin claude/youthful-mayer-qs7e0h && git checkout claude/youthful-mayer-qs7e0h && git pull`.
If your environment names another branch, this one still wins.

**Before you start:** `CHANGELOG.md` has the "Terminal UI" section and the
"Fixes from the front-end review" list (the previous session's work). The gate
passes with 320 tests.

## Read first
- `docs/next/README.md`: the shared rules (they apply here in full).
- `docs/TUI.md` (phase 2, "`@` file completion"), `docs/PARITY.md` (TUI table,
  the row "`@` file completion", status partial), `docs/CLI.md` (interactive
  section), `docs/ARCHITECTURE.md` (C17, "Read limits").
- The code:
  - `crates/forge-agents/src/commands.rs`: custom commands already expand
    `@path`. See the `AT_FILE` regex and the block after `// @file attachments.`
    at the end of `expand`: it reads each existing file (deduplicated,
    `forge_tools::truncate_middle(.., 50_000)`) and appends
    `<file path="...">...</file>` blocks.
  - `crates/forge-core/src/driver.rs`: `Driver::input`, where every front end's
    message goes (plain prompt, slash command, `!shell`).
  - `crates/forge-tools/src/builtin/read.rs`: what the Read tool does with
    text, images, PDFs and notebooks, and its `permission_subject`.
  - `crates/forge-tools/src/files.rs`: `FileState::record_read` (the
    Read-before-Edit rule).
  - `crates/forge-cli/src/tui/app.rs` (`file_menu`, `complete_file`) and
    `tui/mod.rs` (`project_files`): the completion side, already done.

## Today
Typing `@src/main.rs explain this` completes the path in the TUI, but the
model gets only the text `@src/main.rs` and must read the file itself. Only
custom commands attach file contents.

## The work

### 1. One shared attachment function
Move the `@path` logic out of `commands::expand` into a module of its own
(for example `crates/forge-agents/src/attach.rs`):

```rust
pub struct Attachment { pub path: PathBuf, pub kind: Kind, pub text: String }
pub enum Kind { File, Directory, Skipped(String) }
pub fn at_mentions(text: &str, cwd: &Path, allowed: &dyn Fn(&Path) -> Result<(), String>) -> Vec<Attachment>
pub fn render(atts: &[Attachment]) -> String
```

`commands::expand` uses it and keeps its current output for custom commands
(their tests stay green).

Matching rules, each with a test:
- `@path` at the start or after whitespace; trailing `.,:;)!?` is not part of
  the path.
- `@"path with spaces.md"` (quoted).
- `~/` means the home directory (`forge_config::home()`); other paths are
  relative to the session's cwd.
- Emails (`me@example.com`) and `@mentions` with no such file are left alone,
  with no note.
- Each path is attached once.

### 2. Attach on ordinary prompts, in every mode
In `Driver::input`, for a plain text prompt (not a slash command, not
`!shell`), attach what the text mentions. Choose and document:
- **Shape:** keep the user's text as typed. Add the contents as a separate
  text block after it, wrapped in `<system-reminder>` with one
  `<file path="...">` per file, so the transcript, `/rewind` and
  `prompt_points()` still show the prompt as typed. Check
  `forge_engine::Engine::prompt_points`: it skips text that starts with
  `<system-reminder>`.
- **Limits:**
  - per file, the same 50,000 characters (`truncate_middle`);
  - at most 10 files and 200,000 characters in all; past that, a note says
    what was left out;
  - the numbers go in `docs/ARCHITECTURE.md` "Read limits".
- **Kinds:**
  - text files are attached;
  - directories get a short listing (at most 200 entries, `ignore`-aware like
    `tui::project_files`);
  - binary files, images and PDFs get a one-line note telling the model to use
    Read. Don't try to inline them in this prompt.
- **Permissions:** an attachment is a read. It follows the same allow, ask and
  deny decision as the Read tool on that path (deny rules, working
  directories), checked without prompting:
  - `ask` and `deny` mean the file isn't attached, and the note says so
    ("not attached: outside the working directories, use Read");
  - never attach a file that a deny rule would block.
- **Read-before-Edit:** mark attached files with `FileState::record_read`, so
  the model can Edit them without a Read. Only do this if the engine's tool
  context is reachable from the driver; if it isn't, document the decision.
- **Hooks:** `UserPromptSubmit` sees the prompt as typed.
- **Surfaces:** all of them (`-p`, stream-json, REPL, TUI). In stream-json the
  host sends content blocks: only a message that is a single text block (see
  `commands::command_text_any`) is scanned.

### 3. The TUI
After completion, the input shows `@path `, and sending attaches the file. The
scrollback line for the prompt may add a dim "(attached: a.rs, b.rs)". Keep
it simple.

### 4. Tests
- `forge-agents`: matching rules, limits, directory listing, binary note,
  dedup, quoted paths.
- A driver test (`crates/forge-core/src/driver_tests.rs`, `driver_with`):
  - the request's last user message carries the file content;
  - the transcript prompt text is unchanged;
  - a deny rule (`/permissions add deny Read(secret.txt) --scope session`)
    keeps it out;
  - `/rewind` still lists the prompt as typed.
- An end-to-end test in `crates/forge-cli/tests/e2e.rs` with `MockApi`
  (`forge -p "summarize @notes.md"`): the request body contains the file text.
- Custom-command tests in `forge-agents` still pass unchanged.

### 5. Docs
- `docs/PARITY.md`:
  - the TUI row "`@` file completion" becomes done;
  - add a row for `@` mentions in all modes.
- `docs/CLI.md`: a short "`@` mentions" paragraph (all modes, limits,
  permissions).
- `docs/TUI.md` phase 2: drop "The path reaches the model as text".
- `docs/ARCHITECTURE.md` "Read limits": the new limits.
- `CHANGELOG.md`.
- `docs/GOALS.md` priority 7 "Still to build": remove the `@file` item.
- Add a manual check to `docs/CHECKLIST.md`: in a real session,
  `explain @calc.py` answers from the file without a Read call.

## Finish
Gate, commit (`Goals: priority 7`), push, and report what's done, what isn't
and the manual checks you added.
