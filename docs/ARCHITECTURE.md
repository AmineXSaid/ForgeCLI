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
forge-platform                   OS differences: the shell commands run in, process trees, path display
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
- **Always asked:** these tools (and EnterPlanMode) report `needs_user`, so
  they ask whatever allow rules or modes say. There are two exceptions:
  - deny rules deny them, and `dontAsk` denies them;
  - `bypassPermissions` approves them.
- **Mode changes:** a successful EnterPlanMode sets `plan`. A successful
  ExitPlanMode leaves `plan` for `default`, unless the approval's
  `updatedPermissions` already set a mode (e.g. `acceptEdits`).
- **Sub-agents** never get these tools.

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
- `disabledMcpjsonServers` (user or local layer) wins over both, and also
  turns off servers from `mcpServers` in settings. `--mcp-config` and plugin
  servers are chosen per run and aren't affected.
- The project's checked-in settings cannot approve it.
- An unapproved server shows as `disabled` in `system/init`, with a warning
  on stderr.

**The same rule for the endpoint:** `baseUrl`, `openai.baseUrl`,
`apiKeyHelper` and `openai.apiKeyHelper` in the project's checked-in settings
are ignored with a warning (`forge-core/src/endpoint.rs`). Otherwise cloning a
repository could send the user's key to another host, or run a command at
startup without approval.

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

**`/import`** follows the same trust rule:
- servers from the user's own configs (`~/.codex`, `~/.gemini`,
  `~/.cursor`) go to user settings;
- servers shipped in the repository (`.cursor/mcp.json`) go to `.mcp.json`
  and still need approval.

**Turning servers off and on** (`/mcp reconnect|enable|disable <server|all>`,
and the stream-json `mcp_reconnect` / `mcp_toggle` requests):
- Each server's live state (status, client, tool and prompt lists) sits
  behind its own lock, shared with the tool objects made from it. Disabling
  stops the server and hides its tools, prompts, resources and instructions
  at once, in the session and in sub-agents. Enabling or reconnecting starts
  it from the config it was resolved with; the same tool objects work again.
- A tool the server no longer lists is hidden at once. A tool name it didn't
  list when the session was built has no tool object until the session is
  rebuilt (`/reload-plugins`, `/clear`, a new run).
- `disable` saves the name in `disabledMcpjsonServers` in the launch
  project's `.forge/settings.local.json`; `enable` takes it out. A server
  waiting for approval stays off: `enable` never grants trust.
- A restart stops the old process before starting the new one, so calls in
  flight to that server fail. Changes to one server run one at a time.

**`forge mcp serve`** exposes the built-in tools without prompts, since the
client approves, but refuses commands matching dangerous-command patterns.

### C17. Command dispatch and surfaces

**One registry.** `forge_core::commands::BUILTINS` is the only list of
built-in commands: name, aliases, argument hint, description, the surfaces
it runs on, and whether it is immediate. `/help`, `system/init`
`slash_commands`, the `initialize` response's `commands` and the TUI menu
are all generated from it.

**Resolution**, only at the start of a user message:
1. a built-in, by name or alias;
2. a custom command;
3. a skill, or a chain of up to six (`/a /b text`);
4. an MCP prompt (`/mcp__server__prompt`).

A name containing `/` or `\`, a name with a `.` (a file name), or one that
is an existing root path, is an ordinary prompt when no command has that name. Anything else is `Unknown command: /name`. Commands the
reference ties to its vendor's account or cloud are not registered, so they
get the same answer.

**One driver.** `forge_core::Driver` owns the engine and the catalog. Every
front end (print, stream-json, REPL, TUI) passes each input to
`Driver::input`, which returns either a `TurnResult` or `Exit`.
- A command either expands to a prompt for the model, or answers locally.
- A local answer is a `TurnResult` with `num_turns: 0`, no stop reason and
  `is_error` on failure, so every output format and exit status works
  unchanged.
- Commands need no front-end code. A command that needs a choice takes
  its answer as arguments, so it works in `-p`; the TUI draws a picker
  whose rows are those arguments (`commands::picker`).

**Changing the running session.** Settings commands change the live
objects and never rebuild the session:
- the engine handle's runtime (model, effort, thinking, fast mode, permission
  mode);
- the engine config (compaction);
- the tool context's sandbox cell, which sub-agents share;
- the permission engine's rules and directories;
- the system prompt, rebuilt from `PromptSpec`.

`PromptSpec` keeps every input of the system prompt: the replacement or
appended text, the agent prompt, MCP instructions, the output style and the
environment. That way `/output-style`, `/model` and an SDK host's
`initialize` rebuild it without dropping a part. A note for the model (a new
working directory) rides on the next prompt as a system reminder, so the
cached prefix stays intact.

**Saving.** On interactive surfaces, settings commands also write the
default to the same layer the reference uses. In `-p` and stream-json they
don't. A write is mirrored into the loaded layers, and the answer names any
higher-precedence layer that overrides it.

**Switching sessions.** `/clear`, `/resume`, `/branch`, `/cd` and `/reload-*`
replace the session through `Driver::switch`. It rebuilds the session with
`build_session` from the launch options the front end handed over
(`set_rebuild`). Several things carry over:
- the MCP manager, so connections stay up;
- the running model, effort, thinking, fast mode and permission mode;
- the output style and an SDK host's system prompt;
- the cost and usage so far.

`/branch`, `/cd` and `/reload-*` take the conversation from memory
(`Resume::Loaded`, a snapshot of the live state), so they work with
`--no-session-persistence`. The old session gets SessionEnd (except on a
reload) and the new one SessionStart with `source` set to `clear` or
`resume`.

Front ends hold a `Live` handle, never an `EngineHandle`: Ctrl-C and SDK
control requests (`interrupt`, `set_model`, `rewind_files`, ...) always
reach the current engine. After a switch, stream-json hosts get a new
`system/init` with the new session id.

**Rewind.** `/rewind` works on the person's prompts (user text that isn't a
tool result or only a system reminder):
- **Conversation:** truncates the messages and branches the transcript from
  the message before, with a `system/rewind` record so a resume sees the
  same thing.
- **Code:** the file checkpoints of that turn (C4).
- **Partial summaries:** replace a range with a summary note. For
  `summarize-to`, the kept messages are written again after a compact
  boundary under their original uuids, so checkpoints and SDK `rewind_files`
  ids stay valid.

**Tools that come and go.** Some tools are registered once but shown only
while a setting allows them: the Advisor (while `/advisor` names a model)
and later disabled MCP servers. `ToolRegistry` filters on `is_enabled()`
for the tool list *and* for lookups, so a hidden tool can't run even if
the model names it. Showing or hiding one changes the tool list, which
costs one cache miss on the next request.

**Immediate commands** (`/status`, `/usage` with `/cost` and `/stats`,
`/tasks`, `/context`, `/mcp`, `/btw`, and the TUI's `/keybindings` and
`/terminal-setup`) are marked `immediate` in the registry and answer at once,
even while a turn is in progress. `commands::immediate(text, &catalog)`
decides: an immediate built-in, aliases included. Arguments don't change it
(`/mcp reconnect x`, `/tasks stop x` and `/context all` are immediate too),
and built-ins win over custom commands, so the catalog never turns one into
something else.

*The session view* (`forge_core::view::SessionView`). A turn holds the
driver mutably, so these commands never touch it. They read a view that is
cheap to clone (an `Arc` inside) and survives session switches, as `Live`
does. It has two parts, each an `Arc` replaced whole on every update:
- **The driver's** (`ViewState`): session id, cwd, init facts, settings and
  warnings, surface; the engine handle (runtime, permissions); the
  transcript (title, schedule record); the tool context (working
  directories, sandbox policy, background shells); the provider; a hooks
  summary; start time and activity counters; `/btw` exchanges; skill names;
  memory files; subtask rows (live `Arc`s: running state, tool calls, stop
  token); finished subtasks; the shared scheduler; the MCP manager. The
  driver publishes it when idle (`Driver::sync_view`, at the start and end of
  every input and scheduled task, after a switch) and after each turn of an
  input (`record`), so a goal's later turns see the earlier ones.
- **The engine's** (`forge_engine::EngineSnapshot`, behind
  `EngineHandle::snapshot()`): the conversation (`Arc<Vec<Message>>`),
  cleared tool results, system blocks, tool specs, cost, usage, per-model
  usage, context tokens, the configuration `/context` and `/btw` need, and
  the turn in progress (`TurnProgress`: model calls, API time, tool calls so
  far). The engine publishes it after each model call, after each tool batch
  and at the end of a turn; the driver also asks for it when idle. Each
  publish copies the conversation once; readers share it. Sub-agent engines
  don't publish (nothing reads them).

Readers clone the inner `Arc` and drop the lock at once: no guard is held
for long, and none across `.await` (clippy's `await_holding_lock`, on by
default in its `suspicious` group, checks it).

*One implementation.* `/status`, `/usage`, `/tasks`, `/context`, `/mcp` and
`/btw` live in `commands/immediate.rs` and take `&SessionView`. The idle
path (`commands::execute`) syncs the view, runs the same function and syncs
again, so idle output is what it always was. Mid-turn, `/usage` adds the
turn in progress to the totals.

*Deferred effects.* What needs the driver is recorded on the view as an
`Effect` and applied by `sync_view` when the driver is next free (the end of
the turn, or at once when idle; front ends' idle loops wake on
`effect_recorded()`):
- `RefreshMcp`: after `/mcp reconnect|enable|disable`, the servers'
  instructions and the `system/init` facts. The manager itself changes at
  once: it serializes each server's changes, and tool calls see a
  consistent state;
- `SideUsage`: `/btw`'s cost, priced by the engine (`record_side_usage`);
- `SideQuestion`: the `/btw` exchange, kept for the next side question.
  Until it is applied, `SessionView::side_questions` adds it to the
  driver's, so a second `/btw` in the same turn sees the first.

*`/btw` mid-turn* builds its request from the snapshot
(`forge_engine::side_question_request`, shared with the engine's own
requests): same system prompt and tools, `tool_choice: none`. A model reply
whose tool calls have no results yet is left out. It uses the view's
provider and a token of its own, so Esc interrupts the turn, not the side
question. When idle, Ctrl-C cancels it as before.

*Per surface:*
- **TUI:** `App::submit` sends an immediate command at once while busy
  (echoed, not queued). The session task drives `driver.input(..)` (and a
  scheduled task's turn) as a pinned future in `select!` with `rx.recv()`:
  an immediate input is answered from the view on a task of its own
  (`UiEvent::Reply`), anything else waits in a local queue for the turn to
  end. The spinner keeps running.
- **Stream-json:** the stdin reader (`host::read_stdin`) answers an
  immediate command in a `user` message (a single text block) at once, with
  a `result` line in the usual shape (`num_turns: 0`, the current session
  id) plus `"immediate": true`, and never forwards it to the turn loop. A
  host tells it from the running turn's result by that field (the
  reference's public docs don't describe mid-turn commands). Idle or not,
  the reader answers them, so they never queue behind a prompt.
  `mcp_status`, `mcp_reconnect` and `mcp_toggle` control requests don't wait
  either.
- **Line REPL:** reads stdin only between turns, so commands typed during a
  turn run after it, as before.
- **`-p` with one prompt:** unchanged (there is nothing to run beside).

### C18. Goals

`/goal <condition>` sets a condition (at most 4,000 characters) and sends it
as the prompt. After every model turn while the goal is active:

1. The session's model (`goalCheckModel` picks another) checks the
   conversation against the condition. It sees the prompts, replies, tool
   calls and the end of each tool result, each labelled with its tool (the
   newest 60,000 characters), after a list of every failed tool call in the
   session (errors, and sub-agent results that start with `FAILED`). Its
   instructions: the agent's messages are claims, not evidence; failed work
   counts as not done unless redone; details no tool result shows are
   unverified. It answers `{"verdict": "met" | "not_met" | "impossible",
   "evidence": [...], "reason": ...}`. A `met` with no evidence, anything it
   can't parse, and text without JSON all count as `not_met`.
2. **met:** the goal is achieved. **impossible:** it fails, with the reason.
3. **not_met:** the driver starts another turn with a reminder that names
   the reason and the condition.

**Guards.** The loop stops but keeps the goal (paused) when:
- three turns in a row used no tool;
- a turn was interrupted, or a hook blocked the prompt;
- the check itself failed;
- `--max-turns` (counted across the loop) or the budget ran out;
- a transient API error occurred.

The next prompt that reaches the model resumes it; local commands don't.
An error that won't go away by itself (bad credentials, billing, a missing
model, a conversation too long even after compaction; `TurnResult.fatal`)
clears the goal.

**Ending it.** `/goal clear` (or `stop`, `off`, `reset`, `none`, `cancel`)
and `/clear` end it. `/goal` alone shows the status, the checks so far, the
time and the cost.

**Persistence.** Each change is a top-level `goal` record in the transcript,
outside the message chain like the title, so compaction can't drop it. A
resumed session gets back a goal that was still active.

**Output.** Every turn's `result` goes to the host as it finishes, followed
by a `system/goal` event (`status`: `active`, `achieved`, `failed` or
`cleared`, plus the reason). With a single `-p` prompt, the run exits with
status 1 when the goal ends unmet. In text mode, failures and pauses are
warnings on stderr; an achieved goal is reported only with `--verbose`.

`/goal` is refused while `disableAllHooks` is set, as in the reference,
where goals run as an end-of-turn check.

### C19. Scheduled prompts

A session can run prompts later: on a cron schedule (`CronCreate`, or
`/loop` with an interval) or self-paced (`ScheduleWakeup`, or `/loop`
without one). The scheduler is part of the session; `FORGE_DISABLE_CRON=1`
turns it off.

**Tasks:**
- At most 50; each has an 8-character id.
- Cron uses 5 fields in local time: `*`, values, ranges, steps, lists, and
  month and weekday names. When both day of month and day of week are set,
  either one matches.
- Tasks fire only while the session is idle, between turns. A task that
  came due during a turn fires once afterwards, with no catch-up.
- **Jitter**, the same every time for a task id:
  - recurring tasks run up to 30 minutes late (at most half their interval);
  - one-shots set for :00 or :30 run up to 90 seconds early.
- Recurring tasks expire seven days after creation: they fire one last time,
  then are removed.
- Every change writes a top-level `schedule` record. A resumed session gets
  its recurring tasks back; one-shots and wakeups don't survive.

**`/loop [interval] [prompt]`**, parsed in Rust, not by the model:
- The interval is a leading `5m`, `2h`, `1d` or `30s`, or a trailing
  `every 20m` / `every 5 minutes`. `check every PR` has no interval.
- Seconds round up to minutes. An interval cron can't step evenly (`7m`,
  `90m`, `5h`) is rounded to the nearest one that divides its unit, and the
  confirmation says so.
- **With an interval:** a recurring cron task, and the prompt also runs now.
- **Without one:** the prompt runs now with a note asking the model to call
  `ScheduleWakeup` (60-3600 s, with the `/loop` input as the prompt), or
  `ScheduleWakeup {stop: true}` when the work is done. One wakeup is pending
  at a time.
  - If an iteration does neither, a fallback check comes 20 minutes later;
    a second miss ends the loop.
  - An interrupted iteration ends the loop.
- **Without a prompt:** `.forge/loop.md`, else `<config>/loop.md` (up to
  25 KB), else Forge's maintenance prompt.

**Fired prompts:** a custom command, or skills the model may use, expand as
usual. Built-in commands, user-only skills and MCP prompts reach the model as
plain text. Each firing emits `system/scheduled_task`; `/loop` with an
interval emits `system/scheduled`. `/tasks` lists scheduled tasks, and
`/tasks stop <id>` deletes one.

**Front ends.**
- The REPL and stream-json fire due tasks while waiting for input.
- `-p` keeps running after its prompt while tasks are pending. It stops
  when none are left, on Ctrl-C, at `--max-turns` (counted across the run),
  or when the budget is spent.
- `FORGE_TEST_TIME_SCALE` speeds up the scheduler's clock, for tests only.

### C20. Subtasks

`/subtask <task>` forks the conversation into a background sub-agent.

**The fork.**
- The same system prompt, tools (same order and text) and messages as the
  main conversation, then a note saying it is a background subtask, and the
  task. Its first request reads the main conversation's prompt cache.
- Task, AskUserQuestion, EnterPlanMode, ExitPlanMode, CronCreate, CronDelete
  and ScheduleWakeup stay listed but refuse to run.
- It never asks: any permission prompt is denied with a reason. It starts
  with a copy of the main conversation's rules and mode.
- It shares the session's provider, hooks (SubagentStop at the end),
  sandbox, working directories and file checkpoints. Its edits get their own
  checkpoint turn, placed after the prompts made before it started, even
  while a later prompt runs (`FileHistory::add_turn` / `snapshot_in`): a
  `/rewind` to an earlier prompt undoes them, one to a later prompt doesn't,
  and they never count as the running prompt's writes. Its
  transcript goes under the session's `agents/` directory, with a `forkOf`
  record.
- With `--max-budget-usd` it may spend what was left when it started;
  `--max-turns` caps its turn. At most 8 run at once. Background shells it
  started are killed when it ends.

**Handing back**, the next time the session is idle (a front end waiting for
input, or the start of the next input or scheduled task):
- its spend joins the session's;
- the person gets a notice with its report;
- hosts get `system/subtask` (`status` completed, interrupted or error;
  `result`, `num_tool_calls`, `duration_ms`, `total_cost_usd`,
  `transcript_path`);
- its report reaches the model with the next prompt.

Starting one emits `system/subtask` with `status: started`, and the model
hears of it with the next prompt.

**Stopping.** `/tasks` lists subtasks, running and (the last 20) handed back; `/tasks stop <id>` interrupts one,
which is handed back as interrupted. `/clear` and `/resume` stop running
subtasks and pass no report on (their spend still counts); `/branch`, `/cd`
and the reloads keep them. Session end gives them 3 s, then aborts them.

**Front ends.**
- REPL: a notice when one finishes while the prompt waits.
- stream-json: `system/subtask` as soon as one finishes while idle; no turn
  runs by itself.
- `-p`: keeps running while subtasks are out, then hands them all back and
  runs one more turn, whose answer is the run's result.

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
- **`@` mentions** (`forge_agents::attach`, attached by `Driver::input` to
  plain prompts and by custom commands): 50,000 characters per file
  (`truncate_middle`), at most 10 files and 200,000 characters per message,
  directory listings of at most 200 entries; files over 10 MB, images, PDFs
  and binaries (a NUL in the first 8 KB, or not UTF-8) get a one-line note to
  use Read instead. The permission check is Read's decision on the path,
  never a prompt: `ask` and `deny` both leave the file out. Attached files
  are marked read (`FileState::record_read`) before the turn, and forgotten
  again if a UserPromptSubmit hook blocks the prompt. A file's `</file>`,
  `</directory>` and `</system-reminder>` are escaped so it can't close its
  wrapper, and text the injection scan flags (as for Read output) gets a
  note that it comes from the file, not the user.
