You are continuing work on **ForgeCLI**. It is a Rust-native agentic coding
CLI (repo `AmineXSaid/ForgeCLI`) whose behavioural reference is the leading
commercial terminal coding agent. This is prompt **4 of 4** in `docs/next/`.
Work autonomously to the end of the list below. Don't ask me questions; make
reasonable decisions and record them in the docs.

## Branch

Work and push only on **`claude/youthful-mayer-qs7e0h`**:
`git fetch origin claude/youthful-mayer-qs7e0h && git checkout claude/youthful-mayer-qs7e0h && git pull`.
If your environment names another branch, this one still wins.

**Before you start:** prompts 1–3 landed. `CHANGELOG.md` mentions `@`
attachments, immediate commands mid-turn and the TUI screens. If not, stop and
report which is missing.

## Why
The test suite runs only against mocks (`MockProvider`, `MockApi`) and, for
the TUI, a pseudo-terminal. The person who owns this repo runs the real-API
and real-terminal checks by hand from `docs/CHECKLIST.md`. Make that run
complete, quick and unambiguous. **No product features in this prompt.** You
have no real API key. Don't try to run real-API checks; check everything you
can without one.

## Read first
- `docs/next/README.md`: the shared rules (they apply here in full).
- `docs/CHECKLIST.md` (today: setup, 25 numbered checks, a "Terminal UI"
  section), `docs/BASELINE.md`, `FORGE.md` (the `forge-eval` workflow),
  `crates/forge-eval/` (`run`, `compare`, `validate`, `list`), `evals/tasks/`.
- `CHANGELOG.md` and `docs/PARITY.md`, to list what has never been checked
  against the real API. Anything added since the checklist was written is in
  scope.
- `scripts/capture-fixtures.sh`, for the style of repo scripts.

## The work

### 1. A setup script
Add `scripts/check-setup.sh`. It:
- creates the scratch repository the checklist uses (the `calc.py` and
  `test_calc.py` pair from today's setup, plus what new checks need: a file
  for `@` attachments, a `.forge/settings.json` with a hook for the `/hooks`
  screen, ...);
- takes a target directory (default `/tmp/forge-check`) and refuses to touch a
  non-empty directory unless given `--force`;
- uses POSIX `sh` and runs clean under `shellcheck` if it's available.

### 2. Rewrite `docs/CHECKLIST.md` as one ordered run
- Group the checks by what they need:
  - A: print mode only (a key);
  - B: an interactive terminal (a key and a real terminal emulator);
  - C: special setups (tmux, SSH, `NO_COLOR`, a 40-column window).
- Every check has the exact command or keystrokes, what you should see, and
  where to look (`/export`, `/debug` log path, `git diff`, a file).
- Keep the existing 25 checks (renumber them; keep the old numbers in brackets
  for one release) and add:
  - **Prompt 1:** `@file` attaches without a Read call; a deny rule keeps a
    file out.
  - **Prompt 2:** `/usage` and `/btw` during a long turn, in the TUI and in a
    stream-json session (give a small `jq`/`printf` recipe that drives
    `forge -p --input-format stream-json --output-format stream-json`).
  - **Prompt 3:** each screen (`/diff`, `/context`, `/hooks`, `/agents`), a
    custom `keybindings.json`, and `/color` or `/focus` if they were built.
  - **Known gaps from the TUI work:**
    - `/copy` reaches the system clipboard (and inside tmux with
      `set -g set-clipboard on`);
    - Shift+Enter makes a new line in a terminal with the keyboard protocol
      (kitty, WezTerm, Ghostty, foot);
    - `/terminal-setup` describes the terminal correctly;
    - the terminal is restored after an abnormal exit. A panic can't be
      forced from outside, so check `kill -TERM` and closing the terminal
      window, and say plainly that the panic path is covered only by reading
      the code.
- Add a short results table to fill in (check, pass/fail, notes, version) and
  say where differences go (`docs/PARITY.md`).

### 3. Make sure every command in the checklist is valid
- Every flag and subcommand used must parse. Add a test in
  `crates/forge-cli/tests/` that extracts each `forge ...` command line from
  `docs/CHECKLIST.md` (inside backticks or fenced blocks) and runs it with
  `--help` swapped in, or parses its arguments through `clap` with
  `Cli::try_parse_from`, so a renamed flag breaks the build, not the manual
  run.
- Every slash command used must be in `BUILTINS` or a bundled skill (another
  test).
- Run the mock-friendly checks yourself against `forge_test_host::MockApi`
  where the checklist says what the output looks like. Fix the checklist
  wherever it's wrong.

### 4. A baseline how-to
In `docs/BASELINE.md`, add "Recording the first real baseline":
- the exact `forge-eval validate`, `forge-eval run ...` and
  `forge-eval compare ...` commands;
- where results are written;
- what to commit (and what never: keys, raw transcripts with secrets);
- how to read the A/B report.

Check every command against `forge-eval --help` and the code.

### 5. Docs
- `CHANGELOG.md`.
- `docs/GOALS.md` priority 1 (how to set the baseline) and 7 (status).
- `docs/next/README.md`: mark all four prompts done, with the commits.

## Finish
Gate, commit (`Goals: priority 1, priority 7`), push, and report:
- what's done and what isn't;
- the total number of manual checks;
- an estimate of how long the full manual run takes.
