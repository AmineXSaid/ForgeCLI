# Changelog

## 0.1.0 (unreleased)

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
