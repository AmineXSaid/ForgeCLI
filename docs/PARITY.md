# Parity ledger

The target is Claude Code CLI **v2.1.290**.

**Rule:** a row is `done` only when the **Test** column names a test that
exists and passes. A milestone is done only when every row it claims is
`done` or `out`.

**Status values:**

| Status | Meaning |
| --- | --- |
| `done` | Implemented and tested |
| `partial` | Implemented, but with a documented gap |
| `todo` | Not started |
| `out` | Deliberately left out; the reason is given |

**Wire-shape column (stream-json rows only):**

| Value | Meaning |
| --- | --- |
| `inferred` | Built from public documentation and Forge's TypeScript consumer, not observed from the real binary |
| `observed` | Checked against a captured fixture in `docs/fixtures/` |

## Protocol (M2)

| Item | Status | Wire shape | Test | Notes |
| --- | --- | --- | --- | --- |
| `system/init` | todo | inferred | | |
| `assistant` message | todo | inferred | | |
| `user` message (tool results, `tool_use_result`) | todo | inferred | | |
| `stream_event` (`--include-partial-messages`) | todo | inferred | | |
| `result` success / `error_max_turns` / `error_max_budget_usd` / `error_during_execution` | todo | inferred | | |
| `--input-format stream-json` user messages | todo | inferred | | |
| `--replay-user-messages` (`isReplay`) | todo | inferred | | |
| `control_request` `can_use_tool` (CLI → host) | todo | inferred | | Contract C1 |
| `control_request` `initialize` | todo | inferred | | |
| `control_request` `interrupt` | todo | inferred | | Contract C3 |
| `control_request` `set_permission_mode` | todo | inferred | | |
| `control_request` `set_model` | todo | inferred | | |
| `control_request` `set_max_thinking_tokens` | todo | inferred | | |
| `control_request` `mcp_status` | todo | inferred | | |
| `control_request` `rewind_files` | todo | inferred | | Contract C4 |
| `system/compact_boundary` | todo | inferred | | |
| `system/model_fallback` | todo | inferred | | Contract C6 |

## Tools

| Tool | Status | Test | Notes |
| --- | --- | --- | --- |
| Bash | todo | | persistent cwd, timeout, background |
| BashOutput / KillShell (background shells) | todo | | |
| Read | todo | | text, images, PDF, ipynb |
| Write | todo | | |
| Edit | todo | | |
| MultiEdit | todo | | |
| NotebookEdit | todo | | |
| Glob | todo | | |
| Grep | todo | | |
| LS | todo | | |
| TodoWrite | todo | | |
| Task / Agent | todo | | |
| WebFetch | todo | | |
| WebSearch | todo | | server tool `web_search_20260209` |
| ExitPlanMode / EnterPlanMode | todo | | Contract C8 |
| AskUserQuestion | todo | | Contract C8 |
| Skill | todo | | |
| SlashCommand | todo | | |
| ListMcpResources / ReadMcpResource | todo | | |
| TaskCreate / TaskUpdate / TaskList / TaskGet | todo | | |
| EnterWorktree / ExitWorktree | todo | | |

## Engine

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Agent loop (tool use until `end_turn`) | todo | | |
| Parallel batches of read-only tools | todo | `engine::c2_result_order_survives_concurrency` | Contract C2 |
| Serialized permission prompts | todo | | Contract C2 |
| Interrupt | todo | | Contract C3 |
| Retries with backoff (429 / 5xx / 529) | todo | | |
| Fallback model | todo | | Contract C6 |
| `--max-turns` / `--max-budget-usd` | todo | `engine::c7_unknown_model_budget_fails_closed` | Contract C7 |
| Thinking / effort per model | todo | | |
| Prompt-cache breakpoints | todo | | |
| Micro-compaction | todo | `compact::c9_micro_clears_only_eligible_results`, `compact::c9_micro_is_sticky_across_requests` | Contract C9 |
| Auto-compact / `/compact` | todo | `compact::c9_auto_triggers_at_threshold` | Contract C9 |

## Permissions

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Modes: default (manual), acceptEdits, plan, dontAsk, bypassPermissions | todo | | |
| Mode: auto | todo | | Classifier-based upstream. ForgeCLI's version is a rule-based approximation |
| Rule grammar: `Tool`, `Tool(spec)`, `Bash(prefix *)`, path globs, `WebFetch(domain:)`, `mcp__*` | todo | | |
| Precedence deny > ask > allow | todo | | |
| `--allowedTools` / `--disallowedTools` / `--tools` | todo | | |
| `--add-dir` scoping | todo | | |
| Headless auto-deny | todo | | Contract C1 |

## Config, memory, hooks (M3)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Settings layers + `--setting-sources` | todo | | |
| `--settings` file or JSON | todo | | |
| FORGE.md / CLAUDE.md discovery + `@import` | todo | | |
| Hooks: PreToolUse, PostToolUse, PostToolUseFailure, UserPromptSubmit, Stop, SubagentStop, SessionStart, SessionEnd, PreCompact, Notification | todo | | |
| Hook exit-code semantics per event | todo | `hooks::c11_exit2_semantics_per_event` | Contract C11 |
| Hook JSON output (decision, reason, `updatedInput`, `additionalContext`) | todo | `hooks::c11_json_permission_decision` | Contract C11 |

## Sessions (forge-session)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| JSONL transcript | todo | `session::c5_key_collision_keeps_projects_apart` | Contract C5 |
| `additionalDirectories` restored on resume | todo | | Contract C5 |
| System events in JSONL (fallback, compact, microcompact, mode) | todo | | Contracts C5 and C6 |
| `--continue` / `--resume <id>` | todo | | |
| `--fork-session` / `--session-id` | todo | | |
| `--no-session-persistence` | todo | | |
| File checkpoints + rewind | todo | | Contract C4 |

## Agents, skills, commands (M5)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Sub-agents (Task) with built-in types (general-purpose, Explore, Plan) | todo | | |
| Custom agents `agents/*.md`, `--agents`, `--agent` | todo | | |
| Skills `skills/*/SKILL.md` | todo | | |
| Custom slash commands `commands/*.md` | todo | | |
| Output styles: Default, Explanatory, Learning + custom | todo | | |
| Plugins (`--plugin-dir`, directory format) | todo | | |

## MCP (M6)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| stdio client | todo | | |
| Streamable HTTP client | todo | | |
| SSE client | todo | | |
| Tools, resources, prompts (as slash commands) | todo | | |
| `--mcp-config` / `--strict-mcp-config` / `.mcp.json` | todo | | |
| `forge mcp add/remove/list/get` | todo | | |
| `forge mcp serve` | todo | | |

## CLI flags (from `claude --help`)

| Flag | Status | Test | Notes |
| --- | --- | --- | --- |
| `-p/--print`, `--output-format`, `--input-format` | todo | | |
| `--include-partial-messages`, `--replay-user-messages`, `--include-hook-events` | todo | | |
| `--model`, `--fallback-model`, `--effort`, `--betas` | todo | | |
| `--permission-mode`, `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--permission-prompts`, `--permission-prompt-tool` | todo | | |
| `--allowedTools`, `--disallowedTools`, `--tools`, `--add-dir` | todo | | |
| `--system-prompt[-file]`, `--append-system-prompt[-file]`, `--exclude-dynamic-system-prompt-sections` | todo | | |
| `-c/--continue`, `-r/--resume`, `--fork-session`, `--session-id`, `--no-session-persistence`, `-n/--name` | todo | | |
| `--max-turns`, `--max-budget-usd`, `--json-schema` | todo | | |
| `--mcp-config`, `--strict-mcp-config` | todo | | |
| `--settings`, `--setting-sources`, `--bare`, `--safe-mode`, `--restricted` | todo | | |
| `--agents`, `--agent`, `--plugin-dir`, `--disable-slash-commands` | todo | | |
| `--debug`, `--debug-file`, `--verbose`, `-v/--version` | todo | | |
| `-w/--worktree` | todo | `git::c10_worktree_session_listed_from_main_repo` | Contract C10 |
| `--autocompact` | todo | | |
| `--brief`, `--prompt-suggestions`, `--forward-subagent-text` | todo | | |
| `--cloud`, `--teleport`, `--remote-control*`, `--desktop`, `--chrome`/`--no-chrome`, `--ide`, `--from-pr`, `--environment`, `--file`, `--bg`, `--tmux`, `--plugin-url`, `--ax-screen-reader`, `--system-prompt-snapshot` | out | | Need Anthropic cloud, desktop or IDE services, or hosted infrastructure that a personal CLI doesn't have |

## Subcommands

| Command | Status | Test | Notes |
| --- | --- | --- | --- |
| `mcp` | todo | | |
| `doctor` | todo | | |
| `config` (get/set/list/add/remove) | todo | | |
| `agents` (list) | todo | | Background-session management is out |
| `auth`, `setup-token`, `install`, `update`, `gateway`, `ultrareview`, `attach`, `logs`, `stop`, `rm`, `respawn`, `purge`, `import`, `auto-mode`, `plugin` marketplace | out | | Accounts, cloud, distribution or background-session services |

## TUI (M8) and slash commands (M9)

The rows are added when M8 starts.
