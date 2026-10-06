# ForgeCLI goals

## Objective

The goal is the best terminal coding agent per model. Success means the
highest measured task success, at the lowest cost and wall time, with any
given model.

The reference CLI (v2.1.290) is the **compatibility baseline**: its flags,
stream-json protocol, settings, hooks and tools are matched so that ForgeCLI
can replace it. It is not the ceiling. A better harness can beat a weaker one
running the same model, so the harness gets designed, measured and improved as
the product.

## Rule: measure, don't assume

A change that claims to improve the harness must show it on the evaluation
suite (`forge-eval`). It is compared A/B against the previous build, using the
same model, the same tasks and repeated runs.

| Metric | Definition |
| --- | --- |
| Pass rate | Share of tasks whose checker passes (pass@1, averaged over N runs) |
| Cost | USD per task, and per *passed* task |
| Turns | Model calls per task |
| Wall time | Seconds per task |
| Recovery rate | Share of runs that hit a tool or API failure and still pass |
| False finish | Share of runs that report success while their checker fails |

## The five pillars

Each pillar lists what ForgeCLI does about it and what shows that it works.

### 1. Context management

- **Done:** compaction in tiers: sticky micro-compaction of stale tool
  results, summarization near the limit, and compact-and-retry when a prompt is
  too long.
- **To build:**
  - sub-agents, so exploration runs in a separate context and only its result
    comes back;
  - budgets on tool output, so large results are saved to a file and the model
    gets a pointer;
  - no repeated reads: a file that hasn't changed since the model read it
    returns a short "unchanged" note instead of its full text again;
  - memory files and the plan re-attached after compaction.
- **Measured by:** pass rate and cost on long tasks, and context tokens per
  turn.

### 2. Failure recovery

- **Done:** API retries with backoff, fallback models, and interrupt with
  rewind.
- **To build:**
  - tool errors that say what to do next, for example Edit's
    "old_string not found" listing the closest matches in the file;
  - stuck-loop detection: the same failing call repeated, or edits that undo
    each other, trigger a reminder to step back and change approach;
  - recovering from crashes and truncated output (`max_tokens` mid-tool-call).
- **Measured by:** recovery rate, and turns wasted after the first error.

### 3. Verification

- **Done** (contract C12):
  - check commands detected from manifests (Cargo, package.json scripts, Go,
    Python, Make, scripts, Maven/Gradle and more) or set in
    `verification.commands`, and listed in the environment prompt;
  - a built-in check before a turn ends: with changes unchecked since the
    last change, the model gets a reminder (once by default) naming the files
    and the checks;
  - the reminder asks the final message to state what was verified and what
    was not.
  - It is on by default; `FORGE_VERIFY=0` turns it off for A/B runs, and
    `forge-eval` records `verify_reminders` per run.
- **To build:** LSP diagnostics and formatter feedback after edits (the
  OpenCode pattern).
- **Measured by:** false-finish rate and pass rate (tasks `fix-median`,
  `slugify-trap`, `invoice-rounding`).

### 4. Tool design

- **Done:** precise errors, read-before-write, CRLF-safe Edit, parallel
  read-only batches, and results returned in call order.
- **To build:**
  - tool descriptions tuned against the eval suite;
  - structured results everywhere;
  - output that helps the model's next step: counts, truncation markers, and
    what to try when a call fails.
- **Measured by:** turns per task and invalid-call rate.

### 5. Long-horizon execution

- **To build:**
  - a persistent plan and progress notes (the todo list plus a notes file)
    that survive compaction and resume;
  - budget awareness: tokens and cost left;
  - checkpoints the model can roll back to;
  - resumable sessions, and running unattended in headless mode.
- **Measured by:** pass rate on multi-step tasks, and success after a resume.

## Priority order

1. `forge-eval`: the task suite, the runner and A/B reports. **Done.** There
   are 8 validated tasks across the pillars, plus `validate`, `run` and
   `compare`. No measured baseline exists yet: the first `forge-eval run` with
   a real API key sets it.
2. Sub-agents (the Task tool, built-in Explore and Plan agents), to isolate
   context.
3. The verification loop. **Done**, without LSP feedback.
4. Recovery: Edit match suggestions, stuck-loop detection, `max_tokens`
   recovery.
5. Budgets on tool output, and no repeated reads.
6. A plan and notes that survive compaction.
7. Parity features in order of use: MCP, web tools, plan mode and
   AskUserQuestion, slash commands, skills, output styles, then the
   full-screen UI.
8. Remaining flags and polish.
