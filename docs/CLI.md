# ForgeCLI command-line contract

This document is a public interface. Changing documented syntax, exit
statuses, machine output or config precedence is a breaking change and needs
a note in `CHANGELOG.md`.

## Conventions

| Topic | Contract |
| --- | --- |
| Executable | `forge` (and `forge-eval` for evaluation) |
| Help and version | `forge --help`, `forge <command> --help`, `forge help <command>`, `forge --version`. They need no credentials, no network and no config, and print to stdout. |
| Option style | Descriptive long options, with short aliases only for frequent ones (`-p`, `-c`, `-r`, `-n`, `-q`, `-d`, `-v`, `-h`). `--` ends option parsing: `forge -p -- "-v means?"` |
| Streams | **stdout** carries results only: the answer, a JSON document, an NDJSON stream, or `config` output. **stderr** carries diagnostics, warnings, progress and errors. |
| Machine output | `--output-format json` writes exactly one JSON document; `--output-format stream-json` writes NDJSON. Neither ever contains banners, colors or logs. |
| Color | Color is on when stdout and stderr are terminals and `TERM` isn't `dumb`. `NO_COLOR` (non-empty) turns it off, and `FORCE_COLOR` / `CLICOLOR_FORCE` turn it on. `--color auto\|always\|never` overrides all of these. Machine output is never colored. |
| Quiet and verbose | `-q/--quiet`: errors only on stderr. `--verbose`: adds informational diagnostics. Both affect stderr only, never stdout. `-d/--debug` and `--debug-file <path>` turn on debug logs. |
| No input | `--no-input` (or `FORGE_NO_INPUT=1`): Forge never waits for a person. Permission prompts are denied, and interactive mode refuses to start. |
| Confirmations | Forge has no `--yes`. Tool permissions are granted only by `--permission-mode`, `--allowedTools` or settings rules, never by a blanket yes. |
| Secrets | Values whose key looks secret (`*key*`, `*token*`, `*secret*`, `*password*`, `*auth*`, ...) and values that look like credentials (`sk-…`, `Bearer …`) are printed as `<redacted>` in `config` output and diagnostics. |

## Exit statuses

| Status | Meaning |
| --- | --- |
| `0` | Success. This includes a turn interrupted by an IDE host over the control channel. |
| `1` | The run failed: an API error, a tool or agent failure, or a prompt blocked by a hook. Also `config get` on a key that isn't set. |
| `2` | Usage error: an unknown flag, an invalid value, a missing prompt, or a command that needs a terminal (or input) and has none. |
| `3` | Configuration or credentials are missing or invalid: no API key, an unreadable `--settings`, a session that doesn't exist, or an unknown model with a budget. Also `doctor` when it finds a problem. |
| `4` | A limit ended the run: `--max-turns` or `--max-budget-usd`. |
| `130` | Interrupted by Ctrl-C (SIGINT). The current turn's result is still written. |

## Commands

### `forge [prompt]`: interactive session

- **Purpose:** work with the agent in a project, in conversation.
- **Example:** `forge "add a --dry-run flag to the export command"`
- **Input:** the optional first prompt, then lines typed at the terminal.
  `/compact [instructions]` and `/exit` are built in.
- **Output:** the conversation, on stdout. Warnings go to stderr.
- **Needs a terminal on stdin.** Without one, or with `--no-input`, Forge exits
  with status `2` and suggests `-p`.
- **Interrupting:** Ctrl-C interrupts the current turn without killing the
  process; `/exit` or end-of-file quits.
- **Side effects:**
  - files change as the agent's tools allow;
  - a session transcript and file checkpoints are written to the state
    directory.

### `forge -p [prompt]`: print mode, for scripts and CI

- **Purpose:** run one task without supervision and print the result.
- **Examples:**
  - `forge -p "explain src/main.rs"`
  - `git diff | forge -p "review this"`
  - `forge -p --output-format json --max-budget-usd 0.50 "fix the failing test"`
- **Input:** the prompt argument, piped stdin, or both. With both, stdin comes
  first, then the prompt.
- **Output, per `--output-format`:**
  - `text`: on success, the final answer on stdout; on failure, nothing on
    stdout and the error on stderr.
  - `json`: one result document (schema below), plus `exit_code`. If the run
    can't start, one error document instead.
  - `stream-json`: NDJSON. A `system/init` line, then `assistant`, `user` and
    `system` lines (plus `stream_event` lines with
    `--include-partial-messages`), and a `result` line after each turn.
- **Permissions:** tool calls that need approval are denied, and the model is
  told how to allow them (contract C1). They are listed in
  `result.permission_denials`. To allow them, use `--allowedTools`,
  `--permission-mode` or settings rules, or run shell commands in the
  sandbox (`--sandbox workspace-write`, below), where they need no approval.
- **Limits:**
  - `--max-turns N` caps model calls;
  - `--max-budget-usd X` caps spend; it fails closed when the model's pricing
    is unknown;
  - `--fallback-model M` switches models on overload.
- **Interrupting:** Ctrl-C stops the run; the result is written and the exit
  status is `130`.
- **Worktree:** `-w/--worktree [name]` runs the session in
  `.forge/worktrees/<name>`, on a new branch `forge/<name>` (the name is
  generated when left out). The worktree is kept afterwards, and the main
  checkout is untouched.
- **Web:**
  - `WebFetch` answers its `prompt` from the page with the small model
    (`smallFastModel` setting), so whole pages stay out of the context.
  - `WebSearch` uses the provider's web-search server tool.
  - Both need permission. Allow sites with rules like
    `WebFetch(domain:docs.rs)`.

### `forge -p --input-format stream-json --output-format stream-json`: IDE host protocol

- **Purpose:** let an editor or other program drive Forge.
- **Input:** NDJSON on stdin:
  - `user` messages;
  - `control_request` messages: `initialize`, `interrupt`,
    `set_permission_mode`, `set_model`, `set_max_thinking_tokens`,
    `mcp_status`, `rewind_files`;
  - `control_response` answers to Forge's `can_use_tool` requests.
- **Output:** NDJSON on stdout, as in print mode, plus `control_request` and
  `control_response` lines.
- **Permission prompts** go to the host when
  `--permission-prompt-tool stdio` is given, or by default. With
  `--permission-prompts none` or `--no-input` they are denied.
- **Ends** at end-of-file on stdin. `--replay-user-messages` echoes each user
  message with the uuid it was stored under; `rewind_files` takes that uuid.

### `forge doctor`

Checks the local setup:
- settings files;
- credentials (whether they're present, never their value);
- the endpoint;
- git;
- the config and state directories.

Prints one line per check to stdout. Exits `0` when everything passes and `3`
when anything fails. It never makes network calls.

### `forge config list [--origin] | get <key> | paths`

- **`list`:** the merged settings as JSON, with secrets redacted.
- **`list --origin`:** for each top-level key, its value, its source layer and
  its file.
- **`get <key>`:** takes a dotted key (`permissions.defaultMode`) or a JSON
  pointer. It prints strings raw and other values as JSON, and exits `1` when
  the key isn't set.
- **`paths`:** the config, state and cache directories, and the settings files
  that were read.

Read-only. The output goes to stdout.

### `forge mcp ...`: MCP servers

| Command | Does |
| --- | --- |
| `forge mcp add [-s local\|project\|user] [-t stdio\|http\|sse] [-e K=V]... [-H "K: V"]... <name> <command-or-url> [-- args...]` | Saves a server. `local` writes `.forge/settings.local.json` (private), `project` writes `.mcp.json` (shared) and `user` writes `<config>/settings.json` |
| `forge mcp add-json [-s scope] <name> '<json>'` | The same, from a JSON config |
| `forge mcp remove [-s scope] <name>` | Removes it (from every scope when none is given). Exits 1 if it was not found |
| `forge mcp list` | Starts each server and reports `connected, N tools`, `failed: <why>` or `needs approval`. Exits 1 if any failed |
| `forge mcp get <name>` | Config (secret values hidden), status, server info and tools |
| `forge mcp approve <name>` / `--all` | Trusts servers from this project's `.mcp.json` (local settings) |
| `forge mcp serve` | Forge's built-in tools as an MCP server on stdio |

**Config shapes:**
- `{"command": "...", "args": [...], "env": {...}}`
- `{"type": "http", "url": "...", "headers": {...}}`
- `{"type": "sse", "url": "..."}`

Values may use `${VAR}` and `${VAR:-default}`.

**In a session:**
- `--mcp-config <file-or-json>...` adds servers, and `--strict-mcp-config`
  uses only those.
- Server tools appear as `mcp__<server>__<tool>`, and are allowed with rules
  like `mcp__github` or `mcp__github__create_issue`.
- `system/init` lists each server's status.

### `forge completion <bash|zsh|fish|elvish|powershell>`

Prints a completion script generated from the same definitions as `--help`.
For example: `forge completion bash > ~/.local/share/bash-completion/completions/forge`.

### `forge-eval run | compare | validate | list`

Measures the harness; see `evals/README.md`. `run` writes `report.json` and
`report.md` under `evals/results/<label>/` and prints the Markdown report.
`validate` exits non-zero if any task is invalid.

## Machine output schemas

**Result** (`--output-format json`, and the final `result` line of stream-json):

```json
{"type": "result", "subtype": "success|error_during_execution|error_max_turns|error_max_budget_usd",
 "is_error": false, "result": "final answer text", "stop_reason": "end_turn|interrupted|max_turns|...",
 "num_turns": 3, "duration_ms": 8123, "duration_api_ms": 7012, "session_id": "uuid",
 "total_cost_usd": 0.0421, "usage": {"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
 "modelUsage": {"<model>": {"inputTokens": 0, "outputTokens": 0, "costUSD": 0.0}},
 "permission_denials": [{"tool_name": "Bash", "tool_use_id": "…", "tool_input": {}}],
 "errors": [], "structured_output": null, "uuid": "uuid", "exit_code": 0}
```

`exit_code` appears in `json` mode only.

**Error before a run** (`json` and `stream-json`):

```json
{"type": "error", "error": {"message": "what failed", "hint": "the next step, or null", "exit_code": 3}}
```

The same message also goes to stderr for people.

## Configuration

**Precedence, highest first:**
1. Command-line flags.
2. Environment variables (`FORGE_MODEL`, ...).
3. Managed policy (`/etc/forge/managed-settings.json`). An administrator's
   policy beats user files on purpose.
4. `--settings` (a file or a JSON string).
5. Project local settings: `.forge/settings.local.json`.
6. Project settings: `.forge/settings.json`.
7. User settings: `<config>/settings.json`.
8. Built-in defaults.

**Merging:** objects merge, arrays are concatenated, and scalars from a
higher layer replace lower ones. `--setting-sources user,project,local` limits
which file layers load.

**Invalid files** are skipped with a warning on stderr; `forge doctor` reports
them.

**Locations:**

| Kind | Default | Override |
| --- | --- | --- |
| Config (settings, `FORGE.md`, agents, commands) | `$XDG_CONFIG_HOME/forge`, else `~/.config/forge` (Linux), `~/Library/Application Support/forge` (macOS), `%APPDATA%\forge` (Windows) | `FORGE_HOME` |
| State (sessions, file history) | `$XDG_STATE_HOME/forge`, else the platform state or local-data directory | `FORGE_HOME/state` |
| Cache | `$XDG_CACHE_HOME/forge`, else the platform cache directory | `FORGE_HOME/cache` |
| Project | `.forge/` in the project, plus `FORGE.md` files from the root down to the cwd (a directory's `AGENTS.md` when it has no `FORGE.md`). Agents (and later commands, skills and output styles) are also read from the vendor-neutral `.agents/<kind>/`, below `.forge/<kind>/` in precedence | — |

**Environment variables:**

| Variable | Purpose |
| --- | --- |
| `FORGE_API_KEY` | Messages API key |
| `FORGE_AUTH_TOKEN` | Bearer token, for gateways |
| `FORGE_BASE_URL` | API endpoint |
| `FORGE_CUSTOM_HEADERS` | Extra headers, one `Name: value` per line |
| `FORGE_OPENAI_BASE_URL`, `FORGE_OPENAI_API_KEY` | An OpenAI-compatible endpoint instead |
| `FORGE_MODEL` | Default model |
| `FORGE_MAX_OUTPUT_TOKENS` | Per-request output cap |
| `FORGE_MAX_THINKING_TOKENS` | Thinking budget |
| `FORGE_MAX_RETRIES` | API retries (default 3) |
| `FORGE_PROMPTS_DIR` | A local prompt set |
| `FORGE_HOME` | One root for config, state and cache |
| `FORGE_SANDBOX` | Same as `--sandbox` |
| `FORGE_VERIFY` | `0` turns the verification loop off |
| `MCP_TIMEOUT`, `MCP_TOOL_TIMEOUT` | MCP connect and call timeouts, in milliseconds (30 s, 10 min) |
| `FORGE_NO_INPUT` | Same as `--no-input` |
| `FORGE_LOG` | Log filter, with `--debug` |
| `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE` | Color |

## Verification loop

When a turn has changed files and no check has run since the last change, the
model is reminded once to run the project's checks before it finishes, and to
say in its answer what it verified (contract C12 in `docs/ARCHITECTURE.md`).
Each reminder costs one more model call; it's recorded as `system/verification`
in stream-json and in the session.

**Check commands** come from `verification.commands`, or are detected from the
project's manifests: `cargo build` and `cargo test`; the package.json
`typecheck`, `lint`, `build` and `test` scripts, run with the lockfile's
package manager; `go build ./...` and `go test ./...`; pytest or unittest;
`make check` and `make test`; `./build.sh`, `./test.sh` and others. They're
listed in the system prompt's environment section.

| Setting | Default | Meaning |
| --- | --- | --- |
| `verification.enabled` | `true` | `false` turns the loop off (as does `FORGE_VERIFY=0`) |
| `verification.commands` | detected | The project's check commands |
| `verification.maxReminders` | `1` | Reminders per user turn |

## Security

**Dangerous commands are never approved automatically.**
- A Bash command matching a high- or critical-risk pattern always asks (and
  is denied in `-p` runs and in `dontAsk` mode), whatever allow rules,
  `acceptEdits`, `auto` or the sandbox would say. The patterns cover:
  - recursive deletion of `/` or home;
  - disk overwrite;
  - a downloaded script piped to a shell;
  - reverse shells and fork bombs;
  - credential files sent over the network;
  - decoded payloads piped to a shell;
  - startup-file, cron and service persistence;
  - sudoers and setuid changes.
- The prompt says why the command was flagged, and offers no "always allow".
- Deny rules still apply first.
- `--dangerously-skip-permissions` (`bypassPermissions`) still skips the
  check, as its name says.

**Injected instructions in tool output are marked as data.**
- This applies when a file, web page or command output contains text that
  reads like instructions to an AI agent: "ignore previous instructions",
  chat-template tokens, fake system tags, or invisible Unicode tag
  characters.
- Forge then appends a note to that tool result, telling the model the text
  is data and not to follow it. Nothing is removed. This is OWASP LLM01:
  prompt injection.

## Sandbox

`--sandbox <mode>`, `FORGE_SANDBOX` or the `sandbox.mode` setting (in that
order) run each Bash command inside an OS sandbox. The sandbox is separate from
permissions: a sandboxed command is allowed without a prompt, because the
operating system, not a rule, limits what it can touch.

| Mode | Writable | Network |
| --- | --- | --- |
| `off` (default) | everything the user can write | yes |
| `read-only` | the temp directories only | `sandbox.network` (default off) |
| `workspace-write` | the working directories (`--add-dir` included), the temp directories, and `sandbox.writableRoots` | `sandbox.network` (default off) |

**What stays protected, in every mode:**
- `.forge/` inside each writable root (settings, hooks and agents can't be
  rewritten from a command);
- `.git/hooks` and `.git/config`.

**The rules still apply:**
- deny rules still deny;
- paths outside the working directories still ask.

**Backends:**
- Linux: bubblewrap (`bwrap`). Install it with your package manager.
- macOS: `sandbox-exec` with a generated Seatbelt profile. This backend is
  untested.

`forge doctor` reports which backend was found.

**When the backend is missing,** Forge warns once on stderr and commands go
through normal approval. It never runs them unconfined silently.

**When a command fails because of the sandbox,** the tool result says so. The
model can retry it with `dangerouslyDisableSandbox: true`; that call runs
unconfined and goes through normal approval (denied in `-p` unless a rule
allows it).

Settings example:

```json
{ "sandbox": { "mode": "workspace-write", "network": false, "writableRoots": ["/home/me/.cache/pip"] } }
```

## Network behaviour

**Timeouts:**
- connecting: 30 s;
- between reads of a response: 300 s (`FORGE_MAX_RETRIES` controls retries);
- no overall deadline, because long generations stream for minutes.

**Retries:** only failures before any streamed output are retried, at most
`FORGE_MAX_RETRIES` times, with exponential backoff. They cover HTTP 408, 409,
429 and 5xx (honouring `retry-after`) and connection failures.

**Errors** state what failed, the cause, and the next step. For example:
`could not connect to http://…/v1/messages: Connection refused. Check FORGE_BASE_URL, your network connection and proxy settings.`
