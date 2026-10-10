# Working on ForgeCLI

## The rule for every feature

Read `docs/GOALS.md` before implementing anything. ForgeCLI's goal is the best
measured agent harness per model, per dollar and per minute. Matching the
reference CLI is the baseline, not the goal.

For each feature or harness change:

1. **Say what it changes in real use**, before proposing it: for someone
   running ForgeCLI on their own project, at the terminal or in a script, not
   only on a benchmark. A change whose only effect is a benchmark score is not
   proposed.
2. **Name the pillar** it serves (context management, failure recovery,
   verification, tool design, long-horizon execution, or compatibility) and the
   metric it should move.
3. **Add or update an eval task** in `evals/tasks/` that exercises it.
4. **Measure it.** Run `forge-eval` before and after (`forge-eval run`, then
   `forge-eval compare`), or, where no API key is available, state that the
   comparison is still owed.
5. **Write the pillar in the commit message:** `Goals: <pillar> / <metric>`.

Compatibility work also cites its `docs/PARITY.md` row and test.

## Gates before every push

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets   # no warnings
cargo test --workspace
```

## Naming

ForgeCLI uses only Forge names: `FORGE_*` environment variables, `.forge/` and
`FORGE.md` for configuration, and neutral wording in docs. API wire constants
stay confined to `crates/forge-api/src/messages.rs`.
