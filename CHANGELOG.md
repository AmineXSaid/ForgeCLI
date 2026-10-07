# Changelog

## 0.1.0 (unreleased)

### Windows
- Commands run on Windows: Forge finds Git Bash (next to `git.exe`, the
  standard install directories, scoop), else PowerShell; `FORGE_SHELL` picks
  one. Hooks, `apiKeyHelper`, the status line and custom-command `!` lines use
  the same shell. The Bash tool's description and the environment prompt name
  the shell and how to write paths for it.
- No shell is a clear, non-retryable answer naming where Forge looked and the
  fix, a startup warning and a failing `forge doctor` row, instead of an OS
  error the model kept retrying.
- Timeouts, Esc and KillShell stop a command with everything it started (a job
  object). Paths are shown without the `\\?\` prefix. File tools and
  permission rules accept Git Bash paths and compare paths without regard to
  case, as the filesystem does. `forge doctor` finds `git.exe`.

### Auth
- Each endpoint gets only its own key: `FORGE_API_KEY` and `FORGE_AUTH_TOKEN`
  never reach an OpenAI-compatible endpoint, and `FORGE_OPENAI_API_KEY` never
  reaches the Messages API. A key set for the wrong one is named at startup.
- Errors name the URL that answered and the variable the active endpoint
  reads: a 401 from an OpenAI-compatible endpoint points at
  `FORGE_OPENAI_API_KEY`, a 404 or connection failure at the URL variable in
  use.
- A refused key (401/403) exits with `3`, also mid-run and in JSON modes.
- `forge doctor` reports the provider, the URL and where it came from, and
  the key's source, masked; `forge doctor --probe` checks them with one
  request. `/status` shows the same.
- Quoted values are unquoted; an invalid URL or key, or a failing key helper,
  stops startup with a clear message. `openai.apiKeyHelper` is new.
- Project settings can no longer choose the endpoint or run a key helper.

### Models and context
- `modelLimits` (and `FORGE_CONTEXT_WINDOW`) set the context window and output
  cap of models Forge doesn't know; an OpenAI-compatible endpoint's model list
  is read for them too. Without either, Forge says once that it is guessing.
- Compaction measures the request about to be sent, not the last response: a
  batch of large tool results no longer pushes a turn, or a sub-agent, past
  the window. Servers that report no usage get an estimate instead of 0.
- Context-overflow errors are recognised in the wording OpenAI-compatible
  servers use (`maximum context length`, `context_length_exceeded`, ...), so
  the conversation is compacted and retried instead of failing.
- Costs of models without a known price show as unknown (`cost ?`, `$1.20+`,
  `price unknown` in `/usage`) instead of `$0.00`.

### Rate limits
- One limit on model requests in flight for the whole session
  (`maxConcurrentRequests`, default 4): requests wait for a slot instead of
  failing. A 429 halves the limit, says so, waits (honouring `Retry-After`)
  and retries; the limit grows back after a run of successes.
- The OpenAI-compatible provider honours `Retry-After` and
  `FORGE_MAX_RETRIES`, and stops waiting when interrupted.
- `maxParallelAgents` (default 4) caps Task sub-agents running at once.
- A failed sub-agent's result starts with `FAILED:` and tells the model its
  task is not done, so it can't be reported as finished.

### Terminal UI
- `forge` opens a terminal UI when stdin and stdout are terminals: answers
  stream into the terminal's own scrollback, and a live region at the bottom
  holds the spinner, dialogs, queued messages, a multiline input box, the
  `/` command menu and a status line (mode, model, context, cost).
- Permission, question and plan-approval dialogs; Esc and Ctrl-C interrupt;
  Shift+Tab cycles the permission mode; messages typed during a turn are
  queued.
- Prompt history per project directory in `<state>/history.jsonl`; Ctrl+R
  searches it.
- Pickers for `/model`, `/resume`, `/rewind`, `/output-style` and
  `/permissions` typed without an argument; `@` completes project paths.
- UI-only commands: `/theme` (dark, light, none), `/copy [n]` (OSC 52),
  `/keybindings` (also `?`), `/statusline <command>` and `/terminal-setup`.
- `--no-tui` or `FORGE_TUI=0` keeps the line REPL.
- Forge's own marks: a braille spinner (`⠋⠙⠹…`), `•` before answers, `›`
  before tool calls, `↳` before results, `»` and `‖` for the accept-edits and
  plan modes. `/export` and the line REPL use the same marks.
- `/model` lists only the models your endpoint reports (an OpenAI-compatible
  `/models` listing); with the default backend it shows the current model and
  takes any id. Messages name models by id.

### Slash commands
- One command registry for every mode: `/help`, `/status`, `/usage` (`/cost`,
  `/stats`), `/doctor`, `/hooks`, `/memory`, `/skills`, `/agents`,
  `/plugin`, `/mcp`, `/tasks` (`/bashes`), `/release-notes`, `/clear`,
  `/compact` and `/exit`. Unknown commands report `Unknown command: /name`,
  and skills chain (`/a /b text`).
- Settings commands that apply at once: `/model`, `/effort`, `/fast`,
  `/config key=value` (`/settings`), `/output-style`, `/autocompact`,
  `/sandbox`, `/permissions` (`/allowed-tools`) and `/add-dir`. In the REPL
  they also save the default; in `-p` they change only that run, except
  `/config`, which always saves and says which settings layer would override it.
- Session commands: `/rename` (Forge suggests a name), `/export`, `/diff`,
  `/context [all]` and `/debug`, which turns on a session debug log and
  has Forge read it.
- Session commands: `/rewind` (restore code, conversation or both, or
  summarize part of the conversation), `/branch`, `/resume`, `/cd`,
  `/reload-skills`, `/reload-plugins`; `/clear` now starts a new session
  and keeps the old one resumable.
- `/advisor <model>`: an Advisor tool the model can consult; it sees the
  conversation so far, and its cost counts toward the session.
- `/import`: MCP servers and instructions from Codex, Gemini CLI and
  Cursor, planned first and applied with `--yes`, following the MCP trust
  rules.
- `/subtask <task>`: a background agent forked from the conversation (C20).
  Its report comes back with the next prompt; `-p` waits for it.
- `/mcp reconnect|enable|disable <server|all>`: restart a server, or turn it
  off and on. Disabling hides its tools and prompts at once and is saved in
  local settings; stream-json hosts get the same through `mcp_reconnect` and
  `mcp_toggle`.
- `/feedback` (`/bug`, `/share`): a private bug-report bundle on this
  machine, with secrets masked, including those inside free text. Nothing
  is uploaded.
- Bundled skills, in Forge's own words: `/init`, `/code-review` (`/review`),
  `/security-review`, `/simplify`, `/verify`, `/run`, `/run-skill-generator`,
  `/batch`, `/fewer-permission-prompts`, `/update-config`.
- Scheduled prompts (C19): `/loop` with an interval or self-paced, and the
  `CronCreate`, `CronList`, `CronDelete` and `ScheduleWakeup` tools. `-p`
  keeps running while tasks are pending.
- `/goal`: Forge keeps working until a small-model check finds the goal
  met, with guards against loops (C18). `/plan`, `/btw` side questions,
  `/recap`, and `!command` shell mode.
- Fast mode on models that offer it (`fastMode` setting, `/fast`).
- Custom commands, skills, output styles and plugins (`--plugin-dir`).

### `@` mentions
- `@path` in any prompt (TUI, REPL, `-p`, stream-json) attaches the file:
  the prompt stays as typed, and the contents follow in a system reminder.
  `@"quoted paths"`, `~/`, directory listings, and notes for images, PDFs,
  binaries and files the permission rules keep out. Limits: 50,000 characters
  per file, 10 files and 200,000 characters per message. Attached files count
  as read, so the model can edit them at once.
- One implementation (`forge_agents::attach`) serves prompts and custom
  commands. UserPromptSubmit hooks see the prompt without the attachments.
- `/loop` and scheduled prompts attach their `@` mentions too, read afresh
  on each run.
- An attached file can't close its own wrapper, and text that looks like
  instructions to an agent is flagged as data, as for Read output.
- TUI: completion quotes paths with spaces, and a dim `(attached: ...)` line
  follows the prompt.

### Fixes from checking the four together
- A screen that arrives while a permission prompt is open (`/context` typed
  mid-turn) waits for the answer instead of replacing the prompt, which
  denied the tool and interrupted the turn.
- The TUI's `/` menu follows a reload that keeps the session
  (`/reload-skills`, `/hooks add`, `/agents create`): new skills show up.

### The manual check run
- `docs/CHECKLIST.md` is one ordered run of 55 checks in three parts (print
  mode, interactive, special setups), with exact commands, what to see,
  where to look and a results table. The old numbers stay in brackets.
- `scripts/check-setup.sh [dir] [--force]` builds the scratch repository it
  uses.
- Tests keep it honest: every `forge ...` command in it parses, every slash
  command exists, and the print-mode checks a mock can stand in for are run.
- `docs/BASELINE.md` explains how to record the first real `forge-eval`
  baseline; a test checks its commands.

### TUI screens
- Screens: a command typed without arguments can open a scrolling viewer in
  the live region (still an inline viewport, no alternate screen). Up/Down,
  PageUp/PageDown, Home/End scroll; Enter acts on a row; Esc goes back.
- `/diff` lists the changed files with their +/- counts; Enter jumps to a
  file's hunks, coloured by side.
- `/context` is a 10×10 grid coloured by what fills the window, with the
  same numbers as the text form; it opens mid-turn too.
- `/hooks add <Event> <matcher> <command> [--scope ..]` and
  `/hooks remove <Event> <n>` change hooks in settings and reload the
  session; `/hooks` numbers each event's hooks and names their source. The
  TUI's hooks screen adds (a form) and removes (Enter, then confirm).
- `/agents create <name> --description .. [--tools ..] [--model ..]
  [--scope project|user]` writes an agent definition and reloads; the TUI's
  agents screen has a wizard for it.
- Key rebinding: `<config>/keybindings.json` maps keys to named actions
  (`submit`, `historySearch`, `cycleMode`, ...; `none` unbinds).
  `/keybindings` shows the keys in effect; bad entries are warnings.
- `/color <name>` sets the accent colour for this session; `/focus` keeps
  tool calls out of the scrollback until turned off.

### Immediate commands mid-turn
- `/status`, `/usage` (`/cost`, `/stats`), `/tasks`, `/context`, `/mcp` and
  `/btw` answer at once while a turn runs, in the TUI and for stream-json
  hosts, instead of waiting for the turn to end. The spinner keeps going.
- They read a session view the engine and driver publish (after each model
  call and tool batch, and when idle), so mid-turn numbers are current:
  `/usage` counts the turn so far.
- Stream-json marks their `result` lines `"immediate": true`, so hosts can
  tell them from the running turn's result.
- `/btw` mid-turn answers from the conversation so far; its cost and
  exchange, and `/mcp`'s refresh of instructions and `system/init`, are
  applied when the turn ends. A second `/btw` in the same turn sees the
  first.

### Fixes from the front-end review
- A prompt that starts with a file name (`/package.json has the wrong
  version`) is a prompt, not `Unknown command`.
- A stream-json host's `set_model` now reaches the system prompt too, which
  told the new model it was the old one.
- A host's `set_permission_mode: bypassPermissions` is refused unless the
  session was launched in that mode or with
  `--allow-dangerously-skip-permissions`, and always when managed settings
  set `disableBypassPermissionsMode`.
- The REPL said nothing when `--max-turns`, `--max-budget-usd` or a failed
  compaction stopped a turn: these now arrive as error notices, like API
  errors (print mode still reports them once, with the result).
- Secret masking (`forge config`, `/feedback`) now also catches a bearer
  token inside a value whose key looks harmless (custom headers), quoted
  keys in JSON or YAML text (`"api_key": "..."`), private key blocks, and
  more token shapes (Google, npm, GitLab, Stripe, Slack). The `/feedback`
  report and doctor files are masked too.
- `/import codex` no longer fails on a `config.toml` with several `[[x]]`
  entries that share a key.
- An advisor model priced through `modelPricing` counted as free against
  `--max-budget-usd`; its requests are now priced like the session's own.
- A subtask's edits made while a later prompt ran were checkpointed under
  that prompt: `/rewind <that prompt> code` undid them, and the verification
  loop counted them as the prompt's own writes. They now keep their own turn.
- `/tasks` lists subtasks that have finished (the last 20), not only the
  running ones: they were handed back before the list was made.
- In the terminal UI, `/keybindings` and `/terminal-setup` answer at once
  while a turn runs instead of waiting in the queue.
- Ctrl-D during a turn in the terminal UI interrupts it before leaving,
  instead of leaving it running while Forge waits to exit.

### Fixes from the slash-command review
- Scheduling: cron times no longer hang or skip a day across daylight-saving
  changes; the scheduler follows the wall clock after the machine sleeps;
  `/loop 50m` rounds to hourly and `/loop 20h` to daily; a `-p` run stopped
  by a limit while tasks are scheduled exits 4; after stdin ends, host
  permission prompts are refused instead of hanging.
- Goals: Ctrl-C during the goal check stops the loop; a hook's
  `continue: false` pauses it; `--max-turns` and `--max-budget-usd` count
  across the loop, and the check's cost is counted; a first-call credential
  error clears the goal; pauses are reported as `system/goal` events.
- Sessions: `/reload-*`, `/branch` and `/cd` keep the goal, scheduled tasks,
  background shells and the sandbox; rules allowed for the session survive
  `/clear`; `/cd` brings the new directory's memory files; forks keep their
  file checkpoints; `/rewind` skips synthetic messages and finds checkpoints
  for prompts that changed nothing; `--resume` of another directory's session
  is refused (use `--fork-session`); a torn transcript line no longer makes a
  session unresumable; SessionStart runs with source `resume`.
- Settings: a settings file Forge can't parse is never overwritten, and
  writes are atomic; `/permissions remove` can't drop managed or
  command-line rules; `/config` reports its notes and applies
  `verification.enabled` at once; `.forge/settings.local.json` is kept out
  of git; a custom `anthropic-beta` header joins the beta list; fast mode is
  priced at its rate.
- `!command` runs only in the interactive session (on `-p` it is an ordinary
  prompt), can be interrupted, and keeps at most 8 MB of output.

### Agent harness
- Verification loop: changes don't end a turn until checks have run (C12).
- Recovery:
  - Edit "did you mean" hints;
  - a loop guard;
  - continuation after `max_tokens`, with a higher output cap (C13).
- Safety:
  - dangerous-command patterns always ask;
  - injected instructions in tool output are marked as data (C14);
  - an OS sandbox for Bash (`--sandbox`).
- Context:
  - long output is saved to a file and the result points to it;
  - no repeated reads;
  - the task list survives compaction and resume (C15).

### Tools and integrations
- MCP client (stdio, streamable HTTP, SSE), `forge mcp` and `forge mcp serve` (C16).
- WebFetch (small-model answers), WebSearch, AskUserQuestion, plan mode, `--worktree`.
- Sub-agents (the Task tool) with the built-in Explore and Plan agents, plus custom agents.

### Core
- Headless print mode with text, json and stream-json output, and the SDK
  control protocol.
- Sessions:
  - JSONL transcripts;
  - resume and fork;
  - file checkpoints and rewind;
  - compaction.
- `forge-eval`, a task suite measuring pass rate, cost, turns, recovery and
  false finishes.
