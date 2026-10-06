# Evaluation suite

`forge-eval` measures ForgeCLI on these tasks (see `docs/GOALS.md`). Each task
directory has:
- `task.json`: the instruction, a `check` command, and its pillar;
- `repo/`: the starting files;
- `hidden/`: optionally, checks the agent never sees. They are reached via
  `$FORGE_EVAL_TASK_DIR`.

## Running it

```bash
cargo build --release
export FORGE_API_KEY=...            # or FORGE_OPENAI_BASE_URL for another endpoint
target/release/forge-eval run --label baseline --repeat 3 -- --model sonnet --dangerously-skip-permissions
# change the harness, rebuild, then:
target/release/forge-eval run --label my-change --repeat 3 -- --model sonnet --dangerously-skip-permissions
target/release/forge-eval compare evals/results/baseline evals/results/my-change
```

Results go to `evals/results/<label>/`, which is ignored by git. That folder
holds `report.md`, `report.json`, and for each run its workspace, transcript
and stderr.

## Tasks

| Task | Pillar | What it tests |
| --- | --- | --- |
| fix-median | verification | Fix a bug so the visible tests pass; hidden cases catch partial fixes |
| invoice-rounding | verification | Round totals to cents; rounding in the shared helper breaks another test, which only running the suite reveals |
| slugify-trap | verification | Optimize without changing behaviour; hidden edge cases punish skipped tests (false finish) |
| rename-symbol | long-horizon | Rename across modules, an aliased import and a module-qualified call |
| split-module | long-horizon | Split a module into a package and keep the public API |
| env-default | context | Find one bug among many files from a vague report |
| trace-discount | context | Trace a symptom through 40 modules to a casing bug two calls away (exercises sub-agents) |
| build-recovery | recovery | Iterate on a build script's successive errors without editing it |
| crlf-config | tool-design | Edit one CRLF line in a file with a duplicate key elsewhere |
| csv-to-json | tool-design | Write new code to a precise spec checked on hidden input |

Add tasks for every harness change, and name the pillar they exercise.
