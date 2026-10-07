# Manual checks against the real API

The test suite runs against a mock API, so it can't show how a real model
uses Forge's tools and prompts. Run these checks with a real key
(`FORGE_API_KEY`) before a release, in a scratch git repository. Each step
says what you should see. Record any difference in `docs/PARITY.md`.

Set up once:

```bash
mkdir /tmp/forge-check && cd /tmp/forge-check && git init -q
printf 'def add(a, b):\n    return a - b\n' > calc.py
printf 'from calc import add\n\ndef test_add():\n    assert add(2, 3) == 5\n' > test_calc.py
```

## Goals and loops

1. `forge -p "/goal python3 -m pytest -q passes" --output-format stream-json --verbose`
   - Forge fixes `calc.py`, runs the tests, and stops.
   - The last `system/goal` event has `"status": "achieved"`, and the exit status is 0.
2. `forge -p "/goal make the tests pass without touching any file"`
   - The check reports the goal as impossible (`Goal can't be met: ...` on stderr), and the exit status is 1.
3. In `forge`, run `/loop 2m say the time`.
   - It answers at once, then about every 2 minutes (up to a minute late).
   - `/tasks` lists the task as `every 2 minutes`.
   - `/tasks stop <id>` ends it.
4. In `forge`, run `/loop watch for a file named done.txt and stop when it exists`, then
   `touch done.txt` a few minutes later.
   - The model schedules its own checks with ScheduleWakeup.
   - Once the file exists, it stops the loop (`The loop is finished.`).

## Context and history

5. Make two edits through Forge (`add a docstring to add`, then `rename add to plus`).
   - `/rewind` lists both prompts.
   - `/rewind 2 code` restores `calc.py` to its state before the rename; `git diff` shows only the docstring.
6. `/btw what file did you edit last?`
   - It answers with the file name.
   - `/context` shows the same message count as before the question.
7. `/rewind 2 summarize-to` replaces the first prompt's turn with a summary. `/export` shows the summary note, then prompt 2.

## Settings and modes

8. `/fast on` with the default model.
   - The next request carries `speed: fast` (check with `/debug` and `grep '"model request"'` in the log).
   - On Haiku, `/fast on` refuses with the models that offer it.
9. `/effort low`, then a question.
   - `/debug` shows `output_config` with `"effort": "low"`.
   - In the REPL, `<config>/settings.json` now has `"effortLevel": "low"`.
10. `/output-style explanatory`, then a coding question. The answer includes the explanatory style's insight notes.
11. `/sandbox on` (Linux with bubblewrap), then ask Forge to `touch /etc/forge-check`. The command fails inside the sandbox.

## Sessions

12. `/clear first try`, then `/resume`.
    - The list shows "first try".
    - `/resume 1` brings it back with its messages.
13. `/branch experiment`, then a change. `/resume <original id>` shows the original without that change.
14. `/cd ../other-repo` keeps the conversation. `/status` shows the new directory, and the next answer knows about the move.

## Extras

15. `/advisor opus`, then `refactor calc.py into a class and make sure nothing breaks`.
    - Forge calls the Advisor tool at least once before finishing.
    - `/usage` shows the advisor model's cost.
16. `/import` in a machine with `~/.codex/config.toml` MCP servers.
    - It lists them; `/import codex --yes` adds them to `<config>/settings.json`.
    - The next session connects them (`/mcp`).
17. `/feedback the advisor never answers`.
    - A bundle appears in `<state>/feedback/`.
    - Its transcript has no API keys: `grep -r sk- <bundle>` finds nothing.
18. `/code-review high` on a branch with a planted bug (`return a - b`). The report names `calc.py:2` with a failing input.

## Terminal UI

Run these in a real terminal emulator (not through a pipe).

19. `forge`: the prompt box and the status line appear at the bottom. `hello`
    streams an answer, and scrolling up with the mouse or Shift+PageUp shows
    the whole conversation.
20. During a long answer, press Esc. The turn stops and
    `[Request interrupted by user]` is in the transcript (`/export`).
21. Type a second message while a turn runs. It shows as queued and is sent
    when the turn ends.
22. Ask for a shell command in default mode. The permission dialog appears;
    option 2 adds the rule (`/permissions` lists it).
23. Shift+Tab twice shows plan mode. Asking for a change ends with the plan
    dialog, and option 1 switches to accept-edits.
24. `NO_COLOR=1 forge` uses no colours. A 40-column terminal wraps without
    breaking the box.
25. Leave with `/exit`, with Ctrl-D and with Ctrl-C twice. Each time the
    shell works normally afterwards: typed text echoes, and `stty -a` shows
    `icanon echo`.
