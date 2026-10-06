# Changelog

## 0.1.0 (unreleased)

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
- `/goal`: Forge keeps working until a small-model check finds the goal
  met, with guards against loops (C18). `/plan`, `/btw` side questions,
  `/recap`, and `!command` shell mode.
- Fast mode on models that offer it (`fastMode` setting, `/fast`).
- Custom commands, skills, output styles and plugins (`--plugin-dir`).

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
