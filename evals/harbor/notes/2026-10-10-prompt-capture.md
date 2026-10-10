# What the three harnesses send the model

Captured with `evals/harbor/capture-prompts.sh` (files in `evals/harbor/prompts/`):
the first request each one makes for the task "Reply with the single word: done.",
in unattended mode (ForgeCLI `--autonomous`, Forge Code `-p`, Ante
`--yolo --no-session-save --no-skills -p`).

| | ForgeCLI 0.1.0 (ULTRA01) | Forge Code 2.13.21 | Ante 0.2.9 |
|---|---|---|---|
| System prompt | 1059 words | 1659 words (2 system messages) | 951 words |
| Added to the first user message | nothing | `<task>` tags and the date | agent types (143 words) and a `<folder-structure>` listing of the working directory |
| Tools | 19 | 13 | 9 |
| Words in tool descriptions | 1327 | 4498 (`todo_write` 1588, `task` 893, `shell` 597) | 2935 (`Bash` 870, `Agent` 547) |
| Whole first request | ~5.8k tokens | ~12.6k tokens | ~8.6k tokens |
| Output cap sent | `max_tokens` 32000 | `max_completion_tokens` 20480 | none |
| Environment block | cwd, git?, platform, date | OS, cwd, shell, home | cwd, git?, OS, shell, OS version, date |
| Unattended section | yes (`05-autonomous.md`) | no | no (one prompt for both modes) |
| Todo tool | TodoWrite, pushed by the prompt | `todo_write`, pushed hard | none |
| Long commands | Bash kills at 2 min (max 10) unless `run_in_background`; BashOutput to read | `shell` has no timeout or background option in its schema | Bash returns a handle after a wait window (default 10 s, up to 10 min); the command is never killed and its exit arrives as a notification |
| Reply length | "concise" | "concise" | ≤25 words between tool calls, ≤100 at the end |

Ante's tools: Agent, AskUser, Bash, Edit, Glob, Grep, Read, WebFetch, Write.
ForgeCLI's: Bash, BashOutput, KillShell, Glob, Grep, LS, Read, Edit, MultiEdit,
Write, NotebookEdit, TodoWrite, WebFetch, Skill, CronCreate, CronList,
CronDelete, ScheduleWakeup, Task.

## What Ante tells the model that ForgeCLI doesn't

Ante's "Working defaults" and "Task discipline" read like lessons from
benchmark tasks:
- exact output paths, formats and names in the request are requirements;
- the provided inputs, tests, configs and data are the source of truth; never
  invent substitutes or mock data;
- prefer local resources; download only when they're not enough;
- when a tool fails, fix the environment (missing dependency, wrong version),
  don't patch the tool's source;
- before reimplementing something, map its observable contract (commands,
  flags, inputs, outputs, formats, error cases, persisted state) from its
  source and tests, then build the smallest surface that satisfies it;
- when optimizing, measure the baseline first and stop at the threshold;
- on long tasks, write a plausible answer to disk early and improve it in
  place; when time is short, iterate rather than run optional checks;
- after the needed checks pass, don't repeat or widen them without a reason;
- carry the work through and don't end by offering to do what was asked;
- write down what matters from tool results, since old results may be cleared.

## Changes for ForgeCLI, most promising first

1. **Long commands are not killed.** In unattended runs, a Bash call that
   outlives its timeout goes to the background instead of being killed, and
   returns its id and the output so far; its exit is reported with the next
   tool result. Today a 2-minute default kills installs and builds (the
   pytorch-model-cli `pip install`). `crates/forge-tools/src/builtin/bash.rs:11`,
   the kill at `crates/forge-tools/src/shells.rs:173`.
2. **The task rules above**, in our own words, in `05-autonomous.md`; ForgeCLI
   has the verify and don't-give-up parts already.
3. **A listing of the working directory** in the first message, and the shell
   in `<env>`, so the first turn doesn't go to `ls`.
4. **A smaller tool set when unattended**: no CronCreate, CronList, CronDelete,
   ScheduleWakeup or NotebookEdit, which can't help a one-shot run (about 400
   words of descriptions).
5. **Short replies between tool calls** (Ante: ≤25 words), to save output
   tokens and time.

Measurement owed: each change against the reports of the 2026-10-09 subset
jobs, on the same model and tasks. Ante runs without a todo tool; whether
ForgeCLI's TodoWrite push helps is open — a run with `TodoWrite` removed
would answer it.
