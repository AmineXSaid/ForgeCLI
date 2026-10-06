---
name: update-config
description: Change Forge's settings files: permissions, hooks, environment variables, model and more
---
Change Forge's configuration as the user asks, by editing the right settings file.

**Files**, lowest to highest precedence:
- `<config>/settings.json`: the user's settings, for every project. `<config>` is `$FORGE_HOME`, else the platform config directory (for example `~/.config/forge`). Run `forge config paths` to see it.
- `.forge/settings.json`: project settings, checked in and shared.
- `.forge/settings.local.json`: the user's own settings for this project, not checked in.

Objects merge across files, and arrays are concatenated. Pick the narrowest file that fits the request; when unsure, ask.

**Common keys:**
- `model`, `effortLevel`, `fastMode`, `alwaysThinkingEnabled`, `outputStyle`, `smallFastModel`.
- `permissions`: `allow`, `ask` and `deny` lists of rules, plus `defaultMode` (default, acceptEdits, plan, dontAsk, auto) and `additionalDirectories`.
  - A rule is a tool name, optionally with a specifier: `Read`, `Bash(npm run test:*)`, `Edit(src/**)`, `WebFetch(domain:example.com)`.
- `hooks`: an event name mapped to a list of matchers, e.g. `{"PostToolUse": [{"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "npm run fmt", "timeout": 60}]}]}`.
  - Events: PreToolUse, PostToolUse, PostToolUseFailure, UserPromptSubmit, Stop, SubagentStop, SessionStart, SessionEnd, PreCompact, Notification.
  - A hook gets JSON on stdin. Exit code 2 blocks the action and its stderr goes to the model.
- `env`: environment variables for commands Forge runs.
- `mcpServers`, `autoCompactEnabled`, `autoCompactWindow`, `verification` (`enabled`, `commands`), `sandbox` (`mode`, `network`, `writableRoots`), `respondToBashCommands`, `disableAllHooks`.

**Steps:**
1. Read the file before editing it. Create it, with its directory, if it doesn't exist.
2. Change only what was asked; keep everything else. The result must be valid JSON. Check it, for example with `python3 -m json.tool <file>`.
3. Say which file changed and the new value. Mention when a higher-precedence file sets the same key and will win.
4. Settings load when a session starts. Some apply at once through `/config` or `/permissions`; otherwise a new session picks them up.
