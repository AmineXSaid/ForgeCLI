# Parity ledger

Parity is the compatibility baseline, not the goal. `docs/GOALS.md` sets what
ForgeCLI optimizes and the order work is done in. Read it before picking up a
row here.

The target is the reference CLI, version **2.1.290**.

**Rule:** a row is `done` only when the **Test** column names a test that
exists and passes. A milestone is done only when every row it claims is
`done` or `out`.

Test names are `<crate or file>::<test>`; `e2e` is `crates/forge-cli/tests/e2e.rs`.

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
| `system/init` | done | inferred | `e2e::stream_json_golden_basic` |  |
| `assistant` message | done | inferred | `e2e::stream_json_golden_basic` |  |
| `user` message (tool results, `tool_use_result`) | done | inferred | `e2e::host_permission_prompt_allow_and_deny` |  |
| `stream_event` (`--include-partial-messages`) | done | inferred | `e2e::host_controls_mode_model_and_interrupt` |  |
| `result` success / `error_max_turns` / `error_max_budget_usd` / `error_during_execution` | done | inferred | `e2e::max_turns_and_continue`, `e2e::print_errors`, `engine::c7_budget_and_max_turns_stop_the_run` |  |
| `--input-format stream-json` user messages | done | inferred | `e2e::host_permission_prompt_allow_and_deny` |  |
| `--replay-user-messages` (`isReplay`) | done | inferred | `e2e::replay_uuid_drives_rewind_files` |  |
| `control_request` `can_use_tool` (CLI → host) | done | inferred | `e2e::host_permission_prompt_allow_and_deny` | Contract C1 |
| `control_request` `initialize` | partial | inferred | `e2e::host_permission_prompt_allow_and_deny` | `systemPrompt` / `appendSystemPrompt` are applied; SDK callback hooks and SDK MCP servers in `initialize` are not supported yet |
| `control_request` `interrupt` | done | inferred | `e2e::host_controls_mode_model_and_interrupt` | Contract C3 |
| `control_request` `set_permission_mode` | done | inferred | `e2e::host_controls_mode_model_and_interrupt` |  |
| `control_request` `set_model` | done | inferred | `e2e::host_controls_mode_model_and_interrupt` |  |
| `control_request` `set_max_thinking_tokens` | done | inferred | `e2e::host_sets_thinking_tokens` |  |
| `control_request` `mcp_status` | done | inferred | `mcp::session_uses_mcp_tools_and_reports_status` | `{mcpServers: [{name, status, scope, serverInfo, tools, error}]}` |
| `control_request` `rewind_files` | done | inferred | `e2e::replay_uuid_drives_rewind_files` | Contract C4 |
| `system/compact_boundary` | done | inferred | `engine::c9_auto_triggers_at_threshold` | `compact_metadata: {trigger, pre_tokens}` |
| `system/model_fallback` | done | inferred | `engine::c6_fallback_on_overload_for_this_turn_only` | Contract C6 |

## Tools

| Tool | Status | Test | Notes |
| --- | --- | --- | --- |
| Bash | done | `builtin::bash_runs_and_keeps_cwd_inside_project`, `bash_timeout_kills_process_group`, `bash_interrupt_returns_interrupted` | persistent cwd, timeout, background |
| BashOutput / KillShell (background shells) | done | `builtin::background_shell_output_and_kill` |  |
| Read | partial | `builtin::read_numbers_lines_and_pages`, `notebook_edit_replace_insert_delete` | Text, images, notebooks done. PDFs are sent whole: `pages` is passed as a hint, not extracted |
| Write | done | `builtin::write_creates_and_checkpoints` |  |
| Edit | done | `builtin::edit_requires_read_and_unique_match`, `edit_preserves_crlf_and_multiedit_is_atomic` |  |
| MultiEdit | done | `builtin::edit_preserves_crlf_and_multiedit_is_atomic` |  |
| NotebookEdit | done | `builtin::notebook_edit_replace_insert_delete` |  |
| Glob | done | `builtin::glob_and_grep` |  |
| Grep | done | `builtin::glob_and_grep` |  |
| LS | done | `builtin::ls_lists_tree` |  |
| TodoWrite | done | `builtin::todo_write_validates_and_stores` |  |
| Task / Agent | done | `agents::explore_agent_reports_back_in_its_own_context`, `agents::parallel_agents_run_concurrently` | Named `Task` |
| WebFetch | done | `builtin::web_fetch_converts_summarizes_and_reports_redirects`, `web::urls_are_upgraded_and_metadata_hosts_refused`, `html::converts_a_typical_page` | HTML to Markdown, small-model answer to `prompt`, cross-host redirects reported, metadata hosts blocked, 15 min cache, 10 MB cap |
| WebSearch | done | `builtin::web_fetch_converts_summarizes_and_reports_redirects`, `web::search_results_list_their_sources` | Through the provider's `web_search_20250305` server tool (`webSearch.toolType` overrides); not offered with OpenAI-compatible providers. No live test |
| ExitPlanMode / EnterPlanMode | done | `engine::c8_questions_and_plan_approval_with_a_person`, `engine::c8_headless_questions_and_plans_are_answered_by_contract` | Contract C8; approval ends plan mode, or sets the mode the approval names |
| AskUserQuestion | done | `engine::c8_questions_and_plan_approval_with_a_person`, `engine::c8_headless_questions_and_plans_are_answered_by_contract` | Contract C8; the line REPL numbers the options |
| Skill | todo | | |
| SlashCommand | todo | | |
| ListMcpResources / ReadMcpResource | todo | | |
| TaskCreate / TaskUpdate / TaskList / TaskGet | todo | | |
| EnterWorktree / ExitWorktree | todo | | |

## Engine

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Agent loop (tool use until `end_turn`) | done | `engine::tool_loop_reads_file`, `e2e::print_text_and_request_shape` |  |
| Parallel batches of read-only tools | done | `engine::c2_result_order_survives_concurrency` | Contract C2 |
| Serialized permission prompts | done | `engine::c2_prompts_are_serialized` | Contract C2 |
| Interrupt | done | `engine::c3_interrupt_during_stream`, `engine::c3_interrupt_aborts_running_and_pending_tools` | Contract C3 |
| Retries with backoff (429 / 5xx / 529) | done | `e2e::retries_transient_errors` |  |
| Fallback model | done | `engine::c6_fallback_on_overload_for_this_turn_only`, `engine::non_overload_errors_fail_the_turn` | Contract C6 |
| `--max-turns` / `--max-budget-usd` | done | `engine::c7_unknown_model_budget_fails_closed`, `engine::c7_budget_and_max_turns_stop_the_run`, `e2e::max_turns_and_continue` | Contract C7 |
| Thinking / effort per model | done | `request::thinking_per_model` |  |
| Prompt-cache breakpoints | done | `engine::cache_breakpoints_on_system_tools_and_last_message` | System, last tool and last message |
| Micro-compaction | done | `compact::c9_micro_clears_only_eligible_results`, `engine::c9_micro_is_sticky_across_requests` | Contract C9 |
| Auto-compact / `/compact` | done | `engine::c9_auto_triggers_at_threshold`, `engine::manual_compact_passes_instructions`, `engine::prompt_too_long_compacts_and_retries` | Contract C9. `/compact` works in every mode |

## Permissions

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Modes: default (manual), acceptEdits, plan, dontAsk, bypassPermissions | done | `permissions::modes` |  |
| Mode: auto | partial | `permissions::modes` | Rule-based approximation, not a model classifier |
| Rule grammar: `Tool`, `Tool(spec)`, `Bash(prefix *)`, path globs, `WebFetch(domain:)`, `mcp__*` | done | `permissions::rule_parsing`, `path_rules`, `webfetch_domains_and_mcp`, `prefix_rule_respects_word_boundary` |  |
| Precedence deny > ask > allow | done | `permissions::deny_beats_everything_including_bypass`, `ask_rule_overrides_allow_rule`, `compound_deny_matches_any_part` |  |
| `--allowedTools` / `--disallowedTools` / `--tools` | done | `core::builds_from_settings_and_flags`, `e2e::headless_denies_and_reports` |  |
| `--add-dir` scoping | done | `permissions::read_only_inside_working_dirs_is_allowed` |  |
| Headless auto-deny | done | `engine::c1_headless_denies_and_continues`, `e2e::headless_denies_and_reports` | Contract C1 |

## Harness (beyond the baseline)

These rows go past the reference CLI. Each names its pillar from `docs/GOALS.md`.

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Verification loop (contract C12): detected checks, reminder before finishing on unchecked changes | done | `engine::verify_reminds_once_when_changes_are_unchecked`, `engine::verify_accepts_a_check_after_the_last_change`, `engine::verify_sees_shell_writes_through_git_and_failed_checks`, `verify::detects_checks_from_manifests`, `verify::recognizes_check_commands`, `eval::eval_measures_a_pass_and_a_false_finish` | Verification / false-finish rate. Eval task `invoice-rounding` |
| Edit "not found" hints: whitespace-only differences, line-number prefixes, closest window; ambiguous matches list lines (C13) | done | `builtin::edit_failures_point_at_the_nearest_text` | Recovery / turns after first error. Eval task `makefile-tabs` |
| Loop guard: repeated failure, no progress, oscillating edits, error streak (C13) | done | `stuck::repeated_failures_fire_once`, `stuck::same_result_three_times_is_no_progress`, `stuck::edits_that_undo_each_other`, `stuck::a_streak_of_different_failures`, `engine::loop_guard_reminds_after_repeated_failures` | Recovery |
| `max_tokens` recovery: cut-off tool calls answered, text continued, output cap raised (C13) | done | `engine::max_tokens_continues_text_and_answers_cut_off_calls`, `accumulate::invalid_tool_json_is_marked_truncated` | Recovery |
| Dangerous-command patterns force a prompt (C14, the Goose scanner pattern) | done | `threat::flags_dangerous_commands`, `threat::leaves_ordinary_commands_alone`, `permissions::flagged_commands_are_never_approved_automatically` | Safety (OWASP agent risks) |
| Injected instructions in tool output marked as data (C14) | done | `injection::spots_instructions_hidden_in_data`, `injection::ordinary_text_passes`, `engine::injected_instructions_in_tool_output_are_marked` | Safety (OWASP LLM01). Eval task `injected-docs` |
| `AGENTS.md` memory; `.agents/<kind>/` resource directories | done | `config::agents_md_when_no_forge_md`, `agents::vendor_neutral_agents_dir_is_read_and_forge_wins` | Compatibility with other agent CLIs' repos |
| Tool output budgets with full output saved and pointed to (C15) | done | `builtin::long_output_is_saved_in_full_and_pointed_to`, `permissions::read_dirs_are_readable_not_writable` | Context / tokens per turn. Eval task `log-needle` |
| No repeated reads of unchanged content (C15) | done | `builtin::repeated_reads_of_unchanged_content_are_not_resent` | Context. Structured result `file_unchanged` is inferred |
| Task list survives compaction and resume; budget reminder (C15) | done | `engine::the_plan_survives_compaction_and_resume`, `engine::the_model_is_told_when_turns_run_low` | Long-horizon |
| OS sandbox for Bash (`--sandbox`, `FORGE_SANDBOX`, `sandbox.*` settings) | partial | `builtin::sandbox_confines_writes_and_network`, `sandbox::bwrap_arguments`, `sandbox::failure_hints`, `permissions::sandboxed_commands_need_no_prompt_but_rules_still_apply`, `e2e::sandbox_lets_headless_runs_build_without_prompts` | Tool design / safety. The Codex pattern. Linux bubblewrap done; the macOS Seatbelt profile is untested |

## Config, memory, hooks (M3)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Settings layers + `--setting-sources` | done | `config::layers_merge_in_order`, `config::sources_filter_layers` |  |
| `--settings` file or JSON | done | `config::layers_merge_in_order` |  |
| FORGE.md discovery + `@import` | done | `config::discovers_and_imports`, `config::import_cycles_stop` |  |
| Hooks: PreToolUse, PostToolUse, PostToolUseFailure, UserPromptSubmit, Stop, SubagentStop, SessionStart, SessionEnd, PreCompact, Notification | partial | `engine::hooks_in_the_loop`, `engine::user_prompt_hook_can_block` | All events run except Notification, which waits for the TUI; SubagentStop: `agents::unknown_agent_is_rejected_and_subagent_stop_hook_runs` |
| Hook exit-code semantics per event | done | `hooks::c11_exit2_semantics_per_event` | Contract C11 |
| Hook JSON output (decision, reason, `updatedInput`, `additionalContext`) | done | `hooks::c11_json_permission_decision` | Contract C11 |

## Sessions (forge-session)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| JSONL transcript | done | `session::c5_key_collision_keeps_projects_apart`, `session::write_and_resume_round_trip` | Contract C5 |
| `additionalDirectories` restored on resume | done | `core::resume_restores_additional_directories` | Contract C5 |
| System events in JSONL (fallback, compact, microcompact, mode) | done | `engine::c6_fallback_on_overload_for_this_turn_only`, `engine::c9_micro_is_sticky_across_requests`, `engine::c9_auto_triggers_at_threshold` | Contracts C5, C6 and C9 |
| `--continue` / `--resume <id>` | done | `core::continue_and_fork`, `e2e::max_turns_and_continue` |  |
| `--fork-session` / `--session-id` | done | `core::continue_and_fork`, `session::fork_copies_chain_under_new_id` |  |
| `--no-session-persistence` | done | `session::no_persistence_writes_nothing` |  |
| File checkpoints + rewind | done | `session::rewind_restores_and_deletes`, `engine::edits_are_checkpointed_for_rewind`, `e2e::replay_uuid_drives_rewind_files` | Contract C4 |

## Agents, skills, commands (M5)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| Sub-agents (Task) with built-in types (general-purpose, Explore, Plan) | done | `agents::explore_agent_reports_back_in_its_own_context`, `agents::child_edits_are_checkpointed_in_the_parent_turn` | Isolated context; shared rules, prompt lock, checkpoints and budget; no nesting |
| Custom agents `agents/*.md`, `--agents`, `--agent` | done | `agents::markdown_agents`, `agents::json_agents_and_precedence`, `core::sub_agents_are_offered_and_configurable` |  |
| Skills `skills/*/SKILL.md` (Skill tool, `/name`) | done | `skills::discovers_and_loads_skills_on_demand`, `extend::commands_skills_styles_and_plugins` | Names and descriptions up front, bodies on demand; `disable-model-invocation`, `user-invocable` |
| Custom slash commands `commands/*.md` | done | `commands::expands_arguments_commands_and_files`, `commands::loads_namespaced_commands_with_precedence`, `core::commands::tests::parses_commands_skills_paths_and_unknowns`, `extend::commands_skills_styles_and_plugins` | `$ARGUMENTS`, `$1`..`$9`, ``!`cmd` `` gated by `allowed-tools`, `@file`, `dir:name` namespaces. `allowed-tools` does not yet grant tool permissions for the turn, and `model` is not applied |
| Output styles: default, explanatory, learning + custom | done | `styles::builtins_and_custom_styles`, `extend::commands_skills_styles_and_plugins` | `outputStyle` setting; Forge's own style texts |
| Plugins (`--plugin-dir`, `pluginDirs`) | done | `plugins::manifests_and_components`, `extend::commands_skills_styles_and_plugins` | Commands, agents, skills, output styles, hooks and `.mcp.json`. No marketplace (out of scope) |
| Built-in commands in every mode | done | See "Slash commands" below | One registry (C17) |

## MCP (M6)

| Item | Status | Test | Notes |
| --- | --- | --- | --- |
| stdio client (server requests `ping`, `roots/list`; `list_changed`; stderr in errors) | done | `client::stdio_server_tools_resources_and_server_requests`, `client::failures_are_reported_not_fatal`, `transport::dispatch_routes_responses_requests_and_notifications` | |
| Streamable HTTP client (JSON and SSE answers, `Mcp-Session-Id`, protocol header, DELETE on close) | done | `client::streamable_http_json_sse_and_sessions` | |
| SSE client (2024-11-05 transport) | partial | | Built; no test against an SSE server yet |
| Tools (`mcp__<server>__<tool>`), resources (`ListMcpResourcesTool`, `ReadMcpResourceTool`) | done | `client::stdio_server_tools_resources_and_server_requests`, `tools::converts_every_content_type`, `tools::names_are_clean_and_short` | |
| Prompts as slash commands | done | `core::commands::tests::parses_commands_skills_paths_and_unknowns` (routing) | `/mcp__<server>__<prompt> args`; arguments fill the declared ones in order. No end-to-end test with a prompt server yet |
| Server instructions in the system prompt | done | `client::stdio_server_tools_resources_and_server_requests` | |
| `--mcp-config` / `--strict-mcp-config` / `.mcp.json` with approval (C16) | done | `config::project_servers_need_the_users_approval`, `config::precedence_and_strict_mode`, `mcp::project_servers_wait_for_approval` | |
| `${VAR}` / `${VAR:-default}` expansion | done | `config::expands_environment_variables` | |
| `forge mcp add/add-json/remove/list/get/approve` | done | `mcp::add_list_get_remove`, `mcp::project_servers_wait_for_approval` | `approve` is Forge's (the reference asks interactively) |
| `forge mcp serve` | done | `client::forge_serves_its_tools`, `mcp::add_list_get_remove` | Dangerous-command patterns are refused (C14) |
| MCP tools in sub-agents | done | `core` wiring via `AgentRuntime.extra_tools` | Agent tool lists may name `mcp__<server>` |
| OAuth for remote servers | todo | | Headers (`-H`) only for now |

## CLI flags (from the reference `--help`)

| Flag | Status | Test | Notes |
| --- | --- | --- | --- |
| `-p/--print`, `--output-format`, `--input-format` | done | `e2e::print_text_and_request_shape`, `e2e::print_json_and_piped_stdin`, `e2e::stream_json_golden_basic` |  |
| `--include-partial-messages`, `--replay-user-messages`, `--include-hook-events` | partial | `e2e::host_controls_mode_model_and_interrupt`, `e2e::replay_uuid_drives_rewind_files` | `--include-hook-events` is accepted but emits nothing yet |
| `--model`, `--fallback-model`, `--effort`, `--betas` | done | `core::builds_from_settings_and_flags`, `engine::c6_fallback_on_overload_for_this_turn_only` |  |
| `--permission-mode`, `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--permission-prompts`, `--permission-prompt-tool` | done | `core::skip_permissions_flag_beats_settings_mode`, `e2e::host_permission_prompt_allow_and_deny`, `e2e::print_errors` |  |
| `--allowedTools`, `--disallowedTools`, `--tools`, `--add-dir` | done | `core::builds_from_settings_and_flags`, `e2e::headless_denies_and_reports` |  |
| `--system-prompt[-file]`, `--append-system-prompt[-file]`, `--exclude-dynamic-system-prompt-sections` | done | `prompts::replace_append_and_exclude_dynamic` |  |
| `-c/--continue`, `-r/--resume`, `--fork-session`, `--session-id`, `--no-session-persistence`, `-n/--name` | partial | `core::continue_and_fork`, `e2e::max_turns_and_continue` | `--name` is accepted but not shown anywhere until the TUI |
| `--max-turns`, `--max-budget-usd`, `--json-schema` | partial | `e2e::max_turns_and_continue`, `engine::c7_budget_and_max_turns_stop_the_run` | `--json-schema` sends `output_config.format` and parses the result; no retry on invalid output yet |
| `--mcp-config`, `--strict-mcp-config` | done | `mcp::session_uses_mcp_tools_and_reports_status`, `mcp::project_servers_wait_for_approval` | |
| `--settings`, `--setting-sources`, `--bare`, `--safe-mode`, `--restricted` | partial | `config::layers_merge_in_order` | `--restricted` not implemented yet |
| `--agents`, `--agent`, `--plugin-dir` | done | `agents::json_agents_and_precedence`, `core::sub_agents_are_offered_and_configurable`, `extend::commands_skills_styles_and_plugins` | |
| `--disable-slash-commands` | todo | | |
| `--debug`, `--debug-file`, `--verbose`, `-v/--version` | done | `e2e::version_and_help` |  |
| `-w/--worktree` | done | `core::c10_worktree_flag_runs_the_session_in_a_new_worktree`, `git::c10_worktree_session_listed_from_main_repo` | Contract C10. The exit prompt to remove it waits for the TUI |
| `--autocompact` | done | `core::autocompact_values` | Also `autoCompactWindow` and `autoCompactEnabled` in settings |
| `--brief`, `--prompt-suggestions`, `--forward-subagent-text` | todo | | |
| `--cloud`, `--teleport`, `--remote-control*`, `--desktop`, `--chrome`/`--no-chrome`, `--ide`, `--from-pr`, `--environment`, `--file`, `--bg`, `--tmux`, `--plugin-url`, `--ax-screen-reader`, `--system-prompt-snapshot` | out | | Need a vendor's cloud, desktop or IDE services, or hosted infrastructure that a personal CLI doesn't have |

## Subcommands

| Command | Status | Test | Notes |
| --- | --- | --- | --- |
| `mcp` | done | `mcp::add_list_get_remove` | `serve`, `add`, `add-json`, `remove`, `list`, `get`, `approve` |
| `doctor` | partial |  | Basic checks only |
| `config` (get/set/list/add/remove) | partial |  | `list` and `get` only |
| `agents` (list) | todo | | Background-session management is out |
| `auth`, `setup-token`, `install`, `update`, `gateway`, `ultrareview`, `attach`, `logs`, `stop`, `rm`, `respawn`, `purge`, `import`, `auto-mode`, `plugin` marketplace | out | | Accounts, cloud, distribution or background-session services |

## Slash commands (C17)

The reference is the official command list (`/help` of version 2.1.290 and
the commands page). Every built-in sits in one registry,
`crates/forge-core/src/commands/mod.rs` `BUILTINS`, which also feeds `/help`,
`system/init` `slash_commands` and the `initialize` response's `commands`.
The test `core::commands::tests::registry_is_consistent` checks that names
and aliases are unique, sorted and never shadow each other.

Prefix `cmds::` is `crates/forge-cli/tests/commands.rs`.

**Modes:** **P** is `-p` (text, json, stream-json), **R** the line REPL, **T**
the full-screen UI (M8). The REPL and `-p` share one driver
(`forge_core::Driver`), so a command tested in `-p` behaves the same in R.

| Command (aliases) | Status | Modes | Test | Notes |
| --- | --- | --- | --- | --- |
| `/help` | done | PR | `cmds::info_commands_answer_locally`, `core::commands::tests::help_and_catalog_come_from_the_registry` | Built-ins with hints and aliases, then custom commands, skills and MCP prompts |
| `/exit` (`/quit`) | done | PR | `cmds::info_commands_answer_locally` | In `-p` it ends quietly with exit 0 |
| `/clear` (`/reset`, `/new`) | partial | PR | `cmds::info_commands_answer_locally` | Empties the context. Gap: the reference starts a new session id; Forge keeps the id (phase 3, session switching) |
| `/compact [instructions]` | done | PR | `extend::commands_skills_styles_and_plugins`, engine compaction tests (C9) |  |
| `/usage` (`/cost`, `/stats`) | done | PR | `cmds::info_commands_answer_locally`, `cmds::json_output_marks_local_results` | Cost, wall and API time, model calls, prompts, tool calls, tokens per model, context now. No plan limits: Forge has no subscription |
| `/status` | done | PR | `cmds::info_commands_answer_locally` | Version, session and title, directories, model, effort, thinking, mode, output style, sandbox, API key source, settings files, memory, MCP, hooks. Immediate |
| `/doctor` (`/checkup`) | done | PR | `cmds::info_commands_answer_locally` | `forge doctor`'s checks plus session warnings, MCP failures and model pricing |
| `/release-notes` | done | PR | `cmds::info_commands_answer_locally` | `CHANGELOG.md`, embedded at build time |
| `/hooks` | partial | PR | `cmds::info_commands_answer_locally` | Read-only list per event and matcher; the reference's editor dialog is T (M8) |
| `/mcp` | partial | PR | `cmds::info_commands_answer_locally` | Status list. `reconnect`, `enable`, `disable` are phase 1b |
| `/skills` | done | PR | `cmds::info_commands_answer_locally` | Source, who can invoke it, token estimate |
| `/agents` | partial | PR | `cmds::info_commands_answer_locally` | List plus how to add one; the creation wizard is T (M8) |
| `/plugin` | partial | PR | `cmds::bad_commands_fail_with_exit_1` | `list` only. Marketplaces are out (vendor service) |
| `/memory` | partial | PR | `cmds::info_commands_answer_locally` | Lists the files; opening one in `$EDITOR` comes with R/T editing |
| `/tasks` (`/bashes`) | partial | PR | `cmds::info_commands_answer_locally`, `cmds::bad_commands_fail_with_exit_1` | Background shells and `stop <id>`; subagents join in phase 6 |
| Unknown and out-of-scope names | done | PR | `cmds::bad_commands_fail_with_exit_1` | `Unknown command: /name`, exit 1; a path or `a/b` is a prompt |
| Skill chaining `/a /b text` | done | PR | `core::commands::tests::parses_commands_skills_paths_and_unknowns` | Up to 6 skills; the text goes to each |
| `/context [all]` | todo | PRT |  | Phase 1b |
| `/model [model]` | todo | PRT |  | Phase 1b |
| `/effort [level\|auto\|status]` | todo | PRT |  | Phase 1b |
| `/fast [on\|off]` | todo | PRT |  | Phase 1b; only where the model supports fast mode |
| `/config [key=value]` (`/settings`) | todo | PRT |  | Phase 1b |
| `/output-style [style]` | todo | PRT |  | Phase 1b |
| `/autocompact [auto\|tokens]` | todo | PRT |  | Phase 1b |
| `/permissions` (`/allowed-tools`) | todo | PRT |  | Phase 1b |
| `/add-dir <path>` | todo | PRT |  | Phase 1b |
| `/sandbox [on\|off\|mode]` | todo | PRT |  | Phase 1b |
| `/reload-skills`, `/reload-plugins` | todo | PRT |  | Phase 3 (session switching) |
| `/rename [name]` | todo | PRT |  | Phase 1b |
| `/export [file]` | todo | PRT |  | Phase 1b |
| `/diff` | todo | PRT |  | Phase 1b |
| `/debug [description]` | todo | PRT |  | Phase 1b |
| `/plan [description]` | todo | PRT |  | Phase 2 |
| `/goal [condition\|clear]` | todo | PRT |  | Phase 2 |
| `/btw [question]` | todo | PRT |  | Phase 2 |
| `/recap` | todo | PRT |  | Phase 2 |
| `!command` shell mode | todo | PRT |  | Phase 2 |
| `/rewind` (`/checkpoint`, `/undo`) | todo | PRT |  | Phase 3 |
| `/branch [name]` | todo | PRT |  | Phase 3 |
| `/resume` (`/continue`) | todo | RT |  | Phase 3; in `-p` use `--resume` |
| `/cd <path>` | todo | PRT |  | Phase 3 |
| `/loop [interval] [prompt]`, `CronCreate`, `CronList`, `CronDelete`, `ScheduleWakeup` | todo | PRT |  | Phase 4 |
| Bundled skills: `/init`, `/code-review` (`/review`), `/security-review`, `/simplify`, `/verify`, `/run`, `/run-skill-generator`, `/batch`, `/fewer-permission-prompts`, `/update-config` | todo | PRT |  | Phase 5; Forge-written prompts |
| `/subtask`, `/advisor`, `/import`, `/feedback` (`/bug`, `/share`) | todo | PRT |  | Phase 6; `/feedback` writes a local bundle, nothing is uploaded |
| `/theme`, `/color`, `/focus`, `/tui`, `/scroll-speed`, `/statusline`, `/keybindings`, `/terminal-setup`, `/copy` | todo | T |  | M8 |
| `/login`, `/logout`, `/upgrade`, `/usage-credits`, `/rate-limit-options`, `/privacy-settings`, `/passes`, `/stickers`, `/mobile`, `/desktop`, `/chrome`, `/remote-control`, `/remote-env`, `/teleport`, `/web-setup`, `/autofix-pr`, `/schedule`, `/install-github-app`, `/install-slack-app`, `/ultrareview`, `/insights`, `/team-onboarding` and other account, cloud or vendor commands | out | P | `cmds::bad_commands_fail_with_exit_1` | They need the vendor's account or cloud. Each answers `Unknown command` |
| `/vim`, `/pr-comments`, `/ultraplan` | out |  |  | Removed upstream |

## TUI (M8)

The rows are added when M8 starts.
