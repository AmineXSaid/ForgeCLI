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

## Changes for ForgeCLI, judged by what they do in real use

1. **Long commands are not killed** (yes). A Bash call that outlives its
   timeout goes to the background instead of being killed, returns its id
   and the output so far, and its exit is reported with the next tool
   result. In real use builds, installs, test suites and data scripts often
   pass 2 minutes; today the model has to guess the timeout, and a wrong
   guess kills the command, can leave a half-installed environment, and
   costs the whole run again. Both at the terminal and in scripts; a hung
   command stays visible in `/tasks` and can be stopped. Default timeout at
   `crates/forge-tools/src/builtin/bash.rs:11`, the kill at
   `crates/forge-tools/src/shells.rs:173`.
2. **Some of Ante's rules** (yes, trimmed), for every mode, not only
   unattended: the paths and names the user gives are requirements; don't
   invent substitutes or mock data; when a tool fails, fix the environment
   rather than patch the tool or its installed packages; before
   reimplementing something, find out its real interface from its source and
   tests. These prevent failures users meet (a stub that looks like a fix, a
   file written under another name, an edited `site-packages`). Left out:
   "write the artifact early" and "stop at the threshold", which serve time
   limits more than users.
3. **Short updates between tool calls** (yes, small): a number, as Ante has
   (≤25 words), rather than "concise". Users read every line in the
   terminal; less to scroll, fewer output tokens.
4. **A listing of the working directory** in the first message (no). It saves
   one `ls` but stays in the context for the whole session, and in a large
   repository or a home directory it is noise.
5. **No Cron or wakeup tools when unattended** (no). `-p` keeps running until
   scheduled prompts are done (docs/CLI.md, "Scheduled prompts"), so a script
   such as "check CI every 10 minutes until it is green" uses them.

Measurement owed: each change against the reports of the 2026-10-09 subset
jobs, on the same model and tasks. Ante runs without a todo tool; whether
ForgeCLI's TodoWrite push helps is open — a run with `TodoWrite` removed
would answer it.
