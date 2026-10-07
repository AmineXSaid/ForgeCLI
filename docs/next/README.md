# Next work: handoff prompts

Four prompts, one per open item from the last session (`docs/GOALS.md`,
priority 7). Hand each one to a fresh agent session as its whole prompt. They
all work on the same branch, **one session at a time, in this order**: each
prompt starts from the previous one's pushed work.

| # | Prompt | Size | Why this position | Status |
| --- | --- | --- | --- | --- |
| 1 | [`@file` attachments](1-at-file-attachments.md) | small | Used on every prompt; self-contained; closes the one "partial" TUI row in PARITY | Done: `0d427cb`, `6c96651` |
| 2 | [Immediate commands mid-turn](2-immediate-commands.md) | large | Reworks the driver and the session loops (TUI, stream-json). Done before 3 so the TUI screens build on its read-only session view instead of being reworked | Done: `95a2fe9`, `d8e209d` |
| 3 | [TUI editor-style screens](3-tui-screens.md) | large | `/diff`, `/context`, `/hooks`, `/agents` screens, key rebinding; needs 2's session view for live data | Done: `6e9b9c6`, `f03302b`, `b0fa230`, `dd6b0d3`, `9da0d69` |
| 4 | [Real-API check pass](4-real-api-check-pass.md) | small | No features: makes the manual checks complete and quick to run, so it comes after everything it checks | Done: the commit "Manual check run ..." after `9da0d69` |

All four are done. What is left needs a real key and a real terminal: the
run in `docs/CHECKLIST.md` and the first `forge-eval` baseline
(`docs/BASELINE.md`).

## The branch

All four work and push on **`claude/youthful-mayer-qs7e0h`** (it holds all
work so far; `main` is behind it and fast-forwards to it). If a session's
environment names a different branch, the prompt's branch still wins: fetch
it, work on it, push to it.

## Starting a session

Each prompt begins the same way:

1. `git fetch origin claude/youthful-mayer-qs7e0h && git checkout claude/youthful-mayer-qs7e0h && git pull`
2. Check the previous prompt landed: its entry is in `CHANGELOG.md` (see each
   prompt's "Before you start"). If it didn't, stop and report that.
3. Run the gate once to see the starting point (`cargo test --workspace`
   passed 320 tests before prompt 1).

## Rules every prompt repeats

- **Sources:** never use leaked or leak-derived source of any commercial CLI.
  Official public docs and open-source projects are fine.
- **Forge's own wording.** The vendor's company and product names never appear
  in Forge code, prompts, UI text or docs. Env vars are `FORGE_*`, project
  files `.forge/` and `FORGE.md`.
- **No model identifiers** in commits, code or docs. No pull requests unless
  asked. Don't send the user's email anywhere.
- **No real vendor CLI binary**, not even against a mock server. Tests use
  `forge_api::MockProvider` (unit and driver tests) and
  `forge_test_host::MockApi` (end-to-end tests in `crates/forge-cli/tests/`).
- **Locks across awaits:** never hold a std `Mutex`/`RwLock` guard across
  `.await`.
- **Before every push**, all three pass with zero warnings and zero failures:
  `cargo fmt --check`, `cargo clippy --workspace --all-targets`,
  `cargo test --workspace`. Regenerate the stream-json golden only on purpose
  (`FORGE_UPDATE_GOLDEN=1`).
- **Commits:** small, one per feature or fix, each with tests and doc updates
  (PARITY row, CLI.md, CHANGELOG, and TUI.md / ARCHITECTURE.md where they
  apply), each pushed. Messages end with a `Goals:` line naming the priority
  or contract.
- **Decide, don't ask.** Make reasonable decisions and record them in the docs.
- **Finish with a report:** what is done, what is not, and the manual checks
  added to `docs/CHECKLIST.md`.
