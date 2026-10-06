# Baseline (2026-10-06)

## What ForgeCLI is

ForgeCLI is a terminal coding agent written in Rust.
- **Binary:** `forge`, built from a Cargo workspace of 16 crates.
- **Platforms:** Linux and macOS, from source. Windows is untested.
- **Users:** developers working in a terminal; IDE hosts that drive
  `forge -p --input-format stream-json`; CI jobs and scripts.
- **Core workflows:**
  1. An interactive session in a project (`forge`).
  2. A one-shot task (`forge -p "..."`), whose result goes to stdout as text
     or JSON.
  3. An IDE host over stream-json, with permission prompts.
  4. Resuming or continuing a session.
  5. Measuring the harness (`forge-eval`).

## Checks run

- **Workspace:** `cargo build`, `cargo clippy --workspace --all-targets` (no
  warnings) and `cargo test --workspace --no-fail-fast`: 127 tests passed.
- **Commands exercised** in an empty `$HOME` with no credentials:
  - `--help`, `--version`;
  - `-p` with text and with json output;
  - an unknown flag;
  - an unreachable endpoint;
  - `doctor`;
  - interactive mode with stdin piped;
  - `--help | head`.
- **GitHub Actions** has never run: every run fails at startup because of an
  account-level block.

## Findings

### Critical: incorrect behaviour, broken automation, unsafe side effects

1. **Errors go to stdout in text mode, and twice.** `forge -p hi` with no
   credentials prints the error on stderr *and* on stdout. Scripts that
   capture stdout see an error as if it were a result.
2. **One exit code for every failure.** A run that fails, missing credentials
   and a hit limit all exit with `1`, so scripts can't tell them apart. Only
   clap's usage errors are distinct (`2`).
3. **Interactive mode doesn't check for a terminal.** With stdin piped, `forge`
   starts the interactive session, prints a banner and reads prompts and
   permission answers from the pipe.
4. **`forge config list` / `get` print secrets.** Values such as API keys in
   the settings `env` block are printed without redaction.
5. **Closed pipes can panic.** Text output uses `println!`, which panics if the
   reader of the pipe exits early.

### Important: confusing workflows, poor errors, missing contracts

6. **Network errors give no cause and no next step.** For example:
   `error sending request for url (…)`.
7. **Missing credentials are found late.** They're reported only at the first
   API call, as an "API Error", with no pointer to `forge doctor`.
8. **No `--no-input`, `--quiet` or color control (`NO_COLOR`)**, and the
   `--yes` scope isn't documented.
9. **Config and state live in `~/.forge`.** XDG locations are ignored, and
   there's no command to show where a setting came from.
10. **No written CLI contract:** commands, outputs, exit statuses and config
    precedence aren't specified anywhere.
11. **No shell completions and no release artifacts.**
12. **Shell commands aren't sandboxed.** Safety depends on permission
    prompts alone, so you get many prompts or broad allow rules.

### Optional: convenience and polish

13. Interactive mode is a line-based stand-in until the full-screen UI exists.
14. `doctor` checks little: there's no probe of the endpoint, model or
    sandbox.

## What to keep

- The stream-json protocol and the flag names that the reference CLI's IDE
  hosts depend on. They are public interfaces.
- The engine contracts C1–C11, the settings layers, and the session format.
