# ForgeCLI architecture

ForgeCLI is a clean-room Rust reimplementation of the Claude Code CLI's
behaviour. The parity target is **Claude Code v2.1.290** (the 2026-10-06
release). The only sources are public interfaces: `claude --help` (v2.1.291 was
the binary on the build machine and its help output was used as the flag
reference), the documented Agent SDK stream-json protocol, and the Anthropic
Messages API docs. No Claude Code source, leaked or otherwise, is used, and
Anthropic's prompt text is never committed (see "Prompts" below).

## Crates

Dependencies point one way. Nothing lower in the list depends on anything
higher.

```
forge-types                      wire types (Messages API + stream-json protocol)
  ├─ forge-api                   Provider trait, Anthropic SSE client, OpenAI-compatible adapter, mock, model table + pricing
  ├─ forge-config                settings layers, memory files (FORGE.md / CLAUDE.md), resource search paths
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
  matches how Claude Code lays out `~/.claude/projects`. Keys longer than 200
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

## Prompts

ForgeCLI ships its own system prompt and tool descriptions in
`crates/forge-engine/prompts/`. At runtime it can load a different set from
`FORGE_PROMPTS_DIR`: a directory of `system/*.md` and `tools/<ToolName>.md`
files, for example a local, uncommitted copy of a published prompt extraction.
`--system-prompt[-file]` and `--append-system-prompt[-file]` work as their help
text says.

## Configuration names

- **Environment:** `FORGE_*` for ForgeCLI's own settings. `ANTHROPIC_API_KEY`,
  `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL` and
  `ANTHROPIC_CUSTOM_HEADERS` are honoured because they describe the API, not
  the CLI. `CLAUDE_CODE_*` variables are **not** read.
- **Settings files** (later wins):

  | Layer | Paths |
  | --- | --- |
  | user | `~/.forge/settings.json`, then `~/.claude/settings.json` |
  | project | `.claude/settings.json`, then `.forge/settings.json` |
  | local | `.claude/settings.local.json`, then `.forge/settings.local.json` |
  | flag | `--settings` |
  | managed | `/etc/forge/managed-settings.json` |

  `.forge` beats `.claude` within each layer.
- **Resources** (agents, commands, skills, output styles): project wins over
  user. Within a level, `.forge/` beats `.claude/` when two names collide.
- **`forge --version`** prints `<version> (ForgeCLI)`, the same shape as
  `claude --version`.

## Read limits

- **Text:** reads 2,000 lines by default and cuts lines at 2,000 characters.
  `offset` and `limit` select a range.
- **Images** (png, jpg, gif, webp): sent as image blocks, up to 5 MB. Larger
  images are rejected with a message.
- **PDFs:** sent as `document` blocks, up to 32 MB. A PDF over 10 pages needs a
  `pages` range of at most 20 pages, given as a page count, not the raw file.
- **Notebooks (`.ipynb`):** rendered cell by cell with their outputs.
