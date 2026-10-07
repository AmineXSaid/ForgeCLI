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

- **Done** (contract C15):
  - compaction in tiers: sticky micro-compaction of stale tool results,
    summarization near the limit, and compact-and-retry when a prompt is too
    long;
  - sub-agents, so exploration runs in a separate context and only its result
    comes back;
  - budgets on tool output: long Bash output (30k characters), and any other
    tool's output over 50k, keeps its head and tail. The full text is saved
    under the cache directory, which can be read back without a prompt, and
    the result names the file;
  - no repeated reads: reading the same range of unchanged content returns a
    short note. That stops once the earlier result was cleared by compaction;
  - memory files and the task list re-attached after compaction.
- **Measured by:** pass rate and cost on long tasks, and context tokens per
  turn (task `log-needle`).

### 2. Failure recovery

- **Done** (contract C13):
  - API retries with backoff, fallback models, and interrupt with rewind.
  - Edit's "not found" error gives the next step:
    - text that matches except for whitespace, quoted with line numbers;
    - Read's line-number prefixes copied into `old_string`;
    - otherwise the closest window, scored.
    Ambiguous matches name their lines.
  - Stuck-loop detection: the same failing call three times, the same result
    three times, edits that undo each other, or five failures in a row each
    trigger one reminder to change approach.
  - Truncated output:
    - a tool call cut off by `max_tokens` is answered with an error, not
      sent as a failed turn;
    - cut-off text is continued, up to 3 times;
    - the output cap is raised to 64k for the rest of the turn.
- **To build:** recovery after a crash (resume an interrupted turn from the
  transcript).
- **Measured by:** recovery rate, and turns wasted after the first error
  (tasks `build-recovery`, `makefile-tabs`).

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

- **Done:**
  - the task list (TodoWrite) survives compaction: it is re-attached to the
    summary and recorded as `system/todos`. It also survives resume: it is
    rebuilt from the last TodoWrite call or that record;
  - budget awareness: one reminder to wrap up when 3 model calls (of at least
    6) or 15% of the budget are left;
  - resumable sessions, and unattended headless runs.
- **To build:**
  - a notes file;
  - checkpoints the model can roll back to itself.
- **Measured by:** pass rate on multi-step tasks, and success after a resume.

## Priority order

1. `forge-eval`: the task suite, the runner and A/B reports. **Done.** There
   are 13 validated tasks across the pillars, plus `validate`, `run` and
   `compare`. No measured baseline exists yet: the first `forge-eval run` with
   a real API key sets it.
2. Sub-agents (the Task tool, built-in Explore and Plan agents), to isolate
   context.
3. The verification loop. **Done**, without LSP feedback.
4. Recovery: Edit match suggestions, stuck-loop detection, `max_tokens`
   recovery. **Done.**
5. Budgets on tool output, and no repeated reads. **Done.**
6. A plan that survives compaction. **Done** (the task list; a notes file is
   still to build).
7. Parity features in order of use:
   - **Done:** MCP (stdio, HTTP and SSE client, `forge mcp`, `forge mcp
     serve`), web tools, plan mode and AskUserQuestion, skills, output styles.
   - **Mostly done:** slash commands, with one registry, goals (C18) and
     scheduled prompts (C19), and background subtasks (C20).
   - **Mostly done:** the terminal UI (docs/TUI.md): an inline viewport over
     the terminal's scrollback, dialogs, pickers, Ctrl+R, `@` completion
     and `@file` attachments in every mode, immediate commands mid-turn
     (TUI and stream-json),
     `/theme`, `/copy`, `/keybindings`, `/statusline`, `/terminal-setup`.
     Checked against mocks and in a pseudo-terminal; the real-terminal checks
     are in docs/CHECKLIST.md.
   - **Still to build:** key rebinding, `/color`, `/focus`, `/tui`, `/scroll-speed`, and the
     editor-style dialogs (`/hooks`, `/agents` wizard, `/diff` viewer,
     `/context` grid).
8. Remaining flags and polish.
