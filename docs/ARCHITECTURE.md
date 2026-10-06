# ForgeCLI architecture

ForgeCLI is a clean-room Rust agentic coding CLI. Its behaviour follows a
reference CLI, version **2.1.290** (the 2026-10-06 release). The only sources
are public interfaces: the reference CLI's `--help` output (2.1.291 was the
binary on the build machine, used as the flag reference), the documented SDK
stream-json protocol, and the Messages API documentation. No leaked or
proprietary source is used, and no third-party prompt text is committed (see
"Prompts" below).

## Crates

Dependencies point one way. Nothing lower in the list depends on anything
higher.

```
forge-types                      wire types (Messages API + stream-json protocol)
  ├─ forge-api                   Provider trait, Messages API SSE client, OpenAI-compatible adapter, mock, model table + pricing
  ├─ forge-config                settings layers, memory files (FORGE.md), resource search paths
  ├─ forge-permissions           modes, rule grammar, decision engine
  ├─ forge-hooks                 hook events, matchers, command runner
  ├─ forge-session               JSONL transcripts, resume / continue / fork, file checkpoints + rewind
  └─ forge-git                   repo detection, status for the prompt, worktrees
forge-tools                      Tool trait, registry, built-in tools
  ├─ forge-compact               auto-compact, /compact, micro-compaction of stale tool results
  └─ forge-mcp                   MCP client (stdio / HTTP / SSE) and `forge mcp serve`
forge-engine                     the query loop: stream a turn, batch tools, permission → hook → execute → hook, interrupt, budgets, fallback model
forge-agents                     Task sub-agents, custom agents, skills, slash commands, output styles, plugins
forge-core                       composition root: builds an engine from config + flags; ports (PermissionPrompter, EventSink)
  ├─ forge-cli                   the `forge` binary (headless print mode, stream-json host protocol, subcommands)
  └─ forge-tui                   interactive terminal UI
forge-test-host                  test-only SDK host: drives `forge` over stream-json the way an IDE extension does
```

## Behavioural contracts

These are fixed before the engine is written. Each has a named test (see
`PARITY.md`).

### C1. Headless permission prompts

In `-p` mode a tool call that resolves to **ask** goes to whoever answers
prompts:

| Configuration | What happens to an `ask` |
| --- | --- |
| `--permission-prompt-tool stdio` (SDK host), `--permission-prompts host` (default) | A `can_use_tool` `control_request` goes to the host. Its `allow` / `deny` answer is final. |
| `-p` with no host, or `--permission-prompts none` | **Denied automatically.** The model receives an `is_error` `tool_result` that names the tool and says how to allow it (`--allowedTools`, a permission mode, or settings rules). The denial is recorded in `result.permission_denials`. The turn continues; it is not failed. |

Read-only tools inside the working directories are allowed by default rules, so
`forge -p "explain this repo"` works with no flags. `bypassPermissions` skips
the prompt but not explicit **deny** rules.

### C2. Concurrency, permissions and hooks

The tool calls in one assistant message run as follows:

1. They're split into consecutive batches. A batch is either a run of
   concurrency-safe tools (read-only: Read, Glob, Grep, LS, WebFetch, ...) or a
   single unsafe tool.
2. **Within a batch every tool runs its own pipeline concurrently:**
   permission check → `PreToolUse` hook → execute → `PostToolUse` hook.
3. **Permission prompts are serialized** through one async mutex, so a person or
   host never sees two prompts at once. Checks that need no prompt don't take
   the mutex.
4. Results go back in the order of the `tool_use` blocks, all in one user
   message. The executor puts each result into the slot of its `tool_use`
   index, so concurrent tools that finish out of order are put back in order.
   Batches run strictly one after another, so a batch boundary can't reorder
   results either.

   Test: `engine::c2_result_order_survives_concurrency`, which runs
   3×Read, Edit, then 2×Read, with deliberately inverted completion times.

That's model (c) from the design review. Running each pipeline per tool means a
prompt sees the filesystem as it is when that tool is about to run (no
check-everything-first TOCTOU window). Serializing the prompts keeps the
TUI/host dialog to one at a time.

### C3. Interrupt

There is one mechanism for interrupting: a `tokio_util::sync::CancellationToken` per
turn, owned by the engine. Esc in the TUI and the `interrupt` control request
both cancel it.

- **The model stream** is dropped at once. The partial assistant message is
  kept if it has any text or complete tool calls; incomplete tool calls are
  discarded.
- **In-flight tools** are aborted, not awaited:
  - every `Tool::call` gets the token;
  - Bash kills its process group;
  - each aborted call gets a `tool_result` with `is_error: true` and the text
    "Interrupted by user".
- **Tools not yet started** get the same result without running.
- **Background shells** (`run_in_background`) survive an interrupt. Only
  KillShell or session end stops them.
- **The transcript** records the partial assistant message, the interrupted
  tool results, and a synthetic user text "[Request interrupted by user]", so a
  resume shows exactly what happened.
- **In stream-json mode** the turn ends with a `result` (subtype `success`, plus
  `stop_reason: "interrupted"`). The process keeps reading stdin.

### C4. Rewind (file checkpoints)

- **What is captured:** content snapshots, not git. Before the first write to
  a file in each user turn, the edit tools save the file's prior content to
  `~/.forge/file-history/<session-id>/<sha256(path)[..16]>@v<n>`. They do this
  for Write, Edit, MultiEdit and NotebookEdit, and for "file did not exist".
  It works in non-git directories and in `--add-dir` directories.
- **What is not captured:** writes made by Bash (`sed -i`, `git checkout`,
  build output) and writes made outside ForgeCLI. The escape hatch for those is
  git, so `/rewind` warns when the working tree has changes it can't restore.
- **What rewind does:** rewinding to a user message restores every file that
  was checkpointed after that message to its snapshot, and deletes files that
  were created after it. `rewind_files` with `dry_run: true` lists the changes
  without applying them.
- **Files committed after a checkpoint** are still restored from the snapshot;
  rewind never touches git history.

### C5. Session storage

- **Location:** `~/.forge/projects/<project-key>/<session-id>.jsonl`.
- **Project key:** the canonical cwd with every non-alphanumeric character
  replaced by `-` (`/home/u/app` becomes `-home-u-app`). That's readable and
  matches the reference CLI's project layout. Keys longer than 200
  characters are cut to 180 and given a `-<sha256[..12]>` suffix.
- **Index:** `~/.forge/projects/index.json` maps each key to its cwd, so
  `/resume` can show real paths.
- **Format:** one JSON object per line. Each entry has `type`, `uuid`,
  `parentUuid`, `sessionId`, `timestamp`, `cwd`, `gitBranch` and `version`, plus
  either a `message` or a summary/title payload.
- **Writes:** entries are appended and flushed after every event, so a crash
  loses nothing. `--fork-session` copies the chain into a new id.
  `--no-session-persistence` writes nothing.
- **Key collisions:** two different directories can share a key (`/a-b` and
  `/a/b` both become `-a-b`). That's allowed:
  - every entry records the exact `cwd`, and the index maps a key to a *list*
    of cwds;
  - `--continue` and `/resume` filter by exact canonical cwd, never by key
    alone.

  Test: `session::c5_key_collision_keeps_projects_apart`.
- **Extra directories:** the session's `additionalDirectories` (from `--add-dir`
  and `/add-dir`) are stored in a `session_meta` entry each time they change.
  A resume restores them, and any `--add-dir` flags on the resuming command are
  merged in (union). Directories added mid-session apply from the next tool
  call on.
- **System events:** `model_fallback`, `compact_boundary`, `microcompact` and
  `permission_mode` changes are written as `system` entries. A resumed session
  shows why its model, context or mode changed.

### C6. Fallback model

`--fallback-model a,b` switches models in two cases:
- the primary returns **529 / overloaded_error** after its retries (3 by default)
  are used up;
- it returns **404 not_found_error** for the model.

The fallback applies to the rest of that user turn only; each new user turn
starts on the primary again. A switch emits a `system` message with subtype
`model_fallback` naming both models, and the same entry is written to the
session JSONL (C5). 429 doesn't trigger a fallback; it's retried with
`retry-after`.

### C7. Budgets and limits

- **`--max-turns N`** counts assistant API calls in a run. Reaching it ends
  with `error_max_turns`.
- **`--max-budget-usd X`** is checked after each API call. Pricing comes from
  the compiled model table in `forge-api/src/models.rs`, which settings can
  override with `modelPricing`. Exceeding it ends with `error_max_budget_usd`.
- **Unknown models fail closed.** If the active model, or any
  `--fallback-model`, has no price in the table and no `modelPricing`
  override, `--max-budget-usd` refuses to start:
  - the error names the model and the setting to add;
  - in stream-json mode this is an `error_during_execution` result before any
    API call.

  Without a budget, an unknown model costs 0 and `/cost` says pricing is
  unknown. Test: `engine::c7_unknown_model_budget_fails_closed`.

### C8. Headless AskUserQuestion and ExitPlanMode

- **With an SDK host:** both tools go through `can_use_tool`. The host's
  `updatedInput` carries the answers / approval, the same way an IDE answers
  them.
- **With no host (`-p` alone):** AskUserQuestion returns an `is_error` result
  ("no interactive user; proceed with your best judgement and state your
  assumptions"), and ExitPlanMode is denied, which keeps the session in plan
  mode.

### C9. Compaction

ForgeCLI shrinks context in three ways.

**Micro-compaction** clears stale tool results.
- **What is eligible:** a `tool_result` that is all of the following:
  - from a tool whose output can be fetched again (Read, Grep, Glob, LS, Bash,
    BashOutput, WebFetch, MCP read tools);
  - older than the 3 most recent user turns;
  - larger than 8,000 characters.
- **When it runs:** when the last request's context tokens exceed 50% of the
  auto-compact threshold.
- **What it does:**
  - each eligible result's content is replaced with `[Old tool result cleared
    to save context. Re-run the tool if you need it again.]`;
  - `is_error` and the `tool_use_id` stay unchanged;
  - clearing is **sticky and append-only**: the cleared ids are written to the
    JSONL as a `system/microcompact` entry and applied on every later request.
    The cached prefix changes once, not on every turn.
- **What it doesn't change:** the transcript keeps the original content.
  Micro-compaction only rewrites what is sent to the API.

**Auto-compact** summarizes the conversation.
- **The window** is `--autocompact <tokens>` (100k–1M), or `autoCompactWindow`
  in settings, or the model's context window.
- **When it runs:** when the last request's context tokens reach
  `window − min(max_output, 32,000) − 13,000`.
- **What it does:**
  1. runs `PreCompact` hooks with `trigger: "auto"`;
  2. asks the model for a structured summary, with no tools;
  3. starts a new message chain: a compact-boundary `system` entry, then a user
     message holding the summary, then the most recent user turn if one is
     still pending.

**`/compact [instructions]`** does the same with `trigger: "manual"` and
passes the instructions to the summary prompt. It doesn't micro-compact first.

**Both** emit `system/compact_boundary` with `{trigger, pre_tokens}`. The old
messages stay in the JSONL ahead of the boundary, so `--resume` loads only
what follows it.

Tests: `compact::c9_micro_clears_only_eligible_results`,
`compact::c9_micro_is_sticky_across_requests`,
`compact::c9_auto_triggers_at_threshold`.

### C10. Worktrees

`-w/--worktree [name]` creates a worktree:
- **Location:** `<repo>/.forge/worktrees/<name>`. The default name is
  `session-<short id>`.
- **Branch:** `forge/<name>`, started from `HEAD`.

The session runs with the worktree as its cwd, so it's stored under the
worktree's project key (C5). Its `session_meta` entry records
`worktree: {name, path, mainRepo}`, and the index entry carries `mainRepo`.
`/resume` run from the main repo lists sessions from the repo and from all of
its worktrees, labelled. Resuming a worktree session `cd`s into the worktree.

Exiting a worktree session:
- **TUI:** asks whether to keep or remove the worktree.
- **`-p`:** leaves it in place.

A worktree with uncommitted changes is never removed automatically. Test:
`git::c10_worktree_session_listed_from_main_repo`.

### C11. Hook exit codes

**Exit code 0** means success.
- If stdout is a JSON object, it's parsed as hook output.
- For `UserPromptSubmit` and `SessionStart`, plain stdout is added to the
  model's context. For every other event, stdout goes to the debug log only.

**Exit code 2** means block. What that does depends on the event:

| Event | Effect of exit code 2 |
| --- | --- |
| PreToolUse | The tool call is blocked. stderr goes to the model as the `tool_result` error. |
| PostToolUse, PostToolUseFailure | The tool already ran. stderr goes to the model next to the result. |
| UserPromptSubmit | The prompt is blocked and erased. stderr is shown to the user only. |
| Stop, SubagentStop | Stopping is blocked. stderr goes to the model, which keeps working. |
| PreCompact, Notification, SessionStart, SessionEnd | Nothing is blocked. stderr is shown to the user. |

**Any other non-zero code** is a non-blocking error: stderr is shown to the
user and execution continues.

**Time limit:** commands time out after 60 s (`timeout` per hook overrides
this). A timeout counts as a non-blocking error.

**JSON output** may contain:
- `continue: false` (with `stopReason`), which stops the whole run;
- `suppressOutput`, `systemMessage`;
- `decision: "block"` with `reason`, which is equivalent to exit code 2 for the
  event;
- `hookSpecificOutput`, which can hold:
  - `permissionDecision` (`allow`, `deny` or `ask`) with
    `permissionDecisionReason`, PreToolUse only. `allow` skips the prompt but
    not deny rules;
  - `updatedInput`, PreToolUse only;
  - `additionalContext`, for UserPromptSubmit, SessionStart and PostToolUse.

Tests: `hooks::c11_exit2_semantics_per_event` and
`hooks::c11_json_permission_decision`.

### C12. Verification before finishing

A main-agent turn that changed files doesn't end on the model's first
"done" unless a check ran after the last change.

**What counts as a change:**
- a write by an edit tool (Write, Edit, MultiEdit, NotebookEdit), including
  writes made by sub-agents, taken from the file history;
- a shell command not classed as read-only, but only when the git worktree
  fingerprint (status, tracked diff, untracked files' size and mtime) differs
  from the one taken at the turn start or at the last check. Outside git,
  shell writes aren't seen.

**What counts as a check:** a Bash call whose command (after env assignments
and wrappers like `timeout`, `uv run`) starts with:
- a configured or detected check command;
- a well-known build, test or lint tool;
- or runs a `build*`/`test*`/`check*` script, a test file, or one of the files
  changed this turn.

A check that fails still counts as evidence.

**The reminder:** when the model stops with unchecked changes, a meta user
message names the changed files and the project's checks, asks for them to
run, and asks the final answer to say what was and wasn't verified. When
nothing changed after a failed check, a different reminder asks the model to
fix it or say plainly that it fails. Each reminder:
- is recorded and emitted as `system/verification` with
  `{kind, files, shell}` or `{kind, command}`;
- comes before the Stop hook;
- is limited to `verification.maxReminders` per turn (default 1).

**No reminder when:**
- the stop reason is `refusal`;
- the turn is in a sub-agent;
- Bash isn't available;
- rules deny the check command.

Tests: `engine::verify_reminds_once_when_changes_are_unchecked`,
`engine::verify_accepts_a_check_after_the_last_change`,
`engine::verify_sees_shell_writes_through_git_and_failed_checks`.

### C13. Recovery

**Edit errors** (`forge-tools`): when `old_string` isn't found, the error
adds, in this order of preference:
1. `old_string` contains Read's line-number prefixes;
2. the lines match except for whitespace (shown numbered);
3. the most similar window of the same length, at least 60% similar (shown
   numbered);
4. "nothing similar".

When there are several matches, the error lists the line each one starts on.

**Loop guard** (`forge-engine/stuck.rs`): within one user turn, each pattern
below fires at most once. When it fires, a meta user message follows the tool
results, and `system/loop_guard` `{kind, ...}` is recorded and emitted. The
patterns:
- the same tool call (name and input) failing 3 times;
- the same call returning the same result 3 times (TodoWrite excluded);
- an Edit or MultiEdit replacement that reverses an earlier one on the same
  file;
- 5 failed calls in a row.

Denied calls are not counted.

**Truncated output:**
- A tool input that isn't valid JSON when its block stops (the `max_tokens`
  case) is kept, as `{"_truncated_input": ...}`, instead of failing the
  stream. The call is answered with an error explaining how to split the
  work.
- A text reply stopped by `max_tokens` is continued with a meta prompt, up
  to 3 times. The pieces join into one result.
- After the first `max_tokens` stop, the turn's output cap rises to 64,000
  (bounded by the model), unless the user set `FORGE_MAX_OUTPUT_TOKENS`, and
  `system/output_limit` is emitted.

Tests:
- `builtin::edit_failures_point_at_the_nearest_text`;
- `stuck::*`;
- `engine::loop_guard_reminds_after_repeated_failures`;
- `engine::max_tokens_continues_text_and_answers_cut_off_calls`;
- `accumulate::invalid_tool_json_is_marked_truncated`.

### C14. Dangerous commands and injected instructions

**Threat patterns** (`forge-permissions/threat.rs`):
- Every Bash command is scanned.
- A high- or critical-risk match makes `decide` return `Ask`, with
  `Reason::Threat` and no suggestions (`Deny` under `dontAsk`). This happens
  after deny rules and before ask rules, allow rules, mode defaults and the
  sandbox. `bypassPermissions` is exempt.
- Medium-risk matches (history tampering) are reported by `scan_command`
  but not escalated.

**Injection markers** (`forge-tools/injection.rs`):
- After a successful call, and after the PostToolUse hooks, the output of
  every tool except the edit tools and TodoWrite is checked for:
  - phrases aimed at an agent;
  - chat-template tokens;
  - system tags;
  - Unicode tag characters.
- On a match, a note quoting the match is appended to the tool result.
  Known false positive: reading an agent harness's own source, where those
  tags appear as string literals.

Tests:
- `threat::flags_dangerous_commands`;
- `threat::leaves_ordinary_commands_alone`;
- `permissions::flagged_commands_are_never_approved_automatically`;
- `injection::*`;
- `engine::injected_instructions_in_tool_output_are_marked`.

### C15. Context budgets and the plan

**Output budgets:**
- Bash keeps 30,000 characters of stdout and of stderr (`max_output_chars`).
- Any other tool except Read keeps 50,000 characters of text.
- Longer output keeps its head and tail. The full text is written to
  `<cache>/tool-output/<session>/<tool_use_id>-<label>.txt`, and the result
  names that file.
- The permission engine lists that directory in `read_dirs`, so Read and
  Grep on it need no prompt (writing there still asks).

**Repeated reads:**
- `FileState` keeps, per `(path, offset, limit)`, the content hash and the
  tool_use_id of the Read that showed it.
- The same range of the same content returns a short "unchanged" note
  (structured `{"type": "file_unchanged"}`).
- Micro-compaction forgets the views whose results it cleared; full
  compaction forgets all of them.
- Sub-agents have their own `FileState`.

**The plan:**
- After compaction, the TodoWrite list is added to the summary message and
  recorded as `system/todos`.
- `restore` rebuilds the list from the last TodoWrite call after the
  boundary, else from that record.

**Budget reminder:** with `--max-turns` of at least 6, or `--max-budget-usd`,
one reminder per turn when 3 calls (or 15% of the budget) are left.

Tests:
- `builtin::long_output_is_saved_in_full_and_pointed_to`;
- `permissions::read_dirs_are_readable_not_writable`;
- `builtin::repeated_reads_of_unchanged_content_are_not_resent`;
- `engine::the_plan_survives_compaction_and_resume`;
- `engine::the_model_is_told_when_turns_run_low`.

### C16. MCP servers and trust

**Sources**, later ones replacing earlier ones by name:
1. `.mcp.json`;
2. `mcpServers` in settings;
3. `--mcp-config`.

`--strict-mcp-config` and `--bare` read only `--mcp-config`.

**Trust:** a `.mcp.json` server is a command shipped with the repository.
- It starts only when the user's own settings trust it: the user or local
  layer, via `enableAllProjectMcpServers` or `enabledMcpjsonServers`
  (`forge mcp approve`).
- `disabledMcpjsonServers` wins over both.
- The project's checked-in settings cannot approve it.
- An unapproved server shows as `disabled` in `system/init`, with a warning
  on stderr.

**Startup:**
- All servers connect concurrently before the first turn: `initialize`,
  then `notifications/initialized`, then `tools/list` (paged).
- Each has `MCP_TIMEOUT` (default 30 s) to connect. Calls have
  `MCP_TOOL_TIMEOUT` (default 10 min).
- A failing server is reported, with its stderr tail, and never fails the
  session.

**Tools:**
- Server tools are named `mcp__<server>__<tool>`: cleaned to
  `[A-Za-z0-9_-]`, at most 64 characters, with a hash suffix when cut.
- They join after `--tools` filtering and before `--disallowedTools`.
- They always go through permissions, since `readOnlyHint` is trusted only
  for running calls in parallel.
- Their output passes through the output budget (C15) and the injection
  markers (C14).

**Server requests:** the client answers `ping` and `roots/list` (the project
directory). Other server requests (`sampling`, `elicitation`) get "method
not found".

**`forge mcp serve`** exposes the built-in tools without prompts, since the
client approves, but refuses commands matching dangerous-command patterns.

## Prompts

ForgeCLI ships its own system prompt and tool descriptions in
`crates/forge-engine/prompts/`. At runtime it can load a different set from
`FORGE_PROMPTS_DIR`: a directory of `system/*.md` and `tools/<ToolName>.md`
files, for example a local, uncommitted copy of a published prompt extraction.
`--system-prompt[-file]` and `--append-system-prompt[-file]` work as their help
text says.

## Configuration names

- **Environment:** only `FORGE_*` variables are read, the API ones included:
  `FORGE_API_KEY`, `FORGE_AUTH_TOKEN`, `FORGE_BASE_URL`, `FORGE_MODEL`,
  `FORGE_CUSTOM_HEADERS`. Other tools' variables are ignored.
- **Settings files** (later wins):

  | Layer | Paths |
  | --- | --- |
  | user | `~/.forge/settings.json` |
  | project | `.forge/settings.json` |
  | local | `.forge/settings.local.json` |
  | flag | `--settings` |
  | managed | `/etc/forge/managed-settings.json` |
- **Resources** (agents, commands, skills, output styles) live in
  `~/.forge/<kind>/` and `.forge/<kind>/`. Project wins over user when two
  names collide.
- **Memory files:** `~/.forge/FORGE.md`, then `FORGE.md` or `.forge/FORGE.md` in
  each directory from the filesystem root down to the cwd, each followed by its
  `FORGE.local.md`.
- **`forge --version`** prints `<version> (ForgeCLI)`.

## Read limits

- **Text:** reads 2,000 lines by default and cuts lines at 2,000 characters.
  `offset` and `limit` select a range.
- **Images** (png, jpg, gif, webp): sent as image blocks, up to 5 MB. Larger
  images are rejected with a message.
- **PDFs:** sent as `document` blocks, up to 32 MB. A PDF over 10 pages needs a
  `pages` range of at most 20 pages, given as a page count, not the raw file.
- **Notebooks (`.ipynb`):** rendered cell by cell with their outputs.
