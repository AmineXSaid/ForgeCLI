You are continuing work on **ForgeCLI**. It is a Rust-native agentic coding
CLI (repo `AmineXSaid/ForgeCLI`) whose behavioural reference is the leading
commercial terminal coding agent. This is prompt **2 of 4** in `docs/next/`.
Work autonomously to the end of the list below. Don't ask me questions; make
reasonable decisions and record them in the docs.

## Branch

Work and push only on **`claude/youthful-mayer-qs7e0h`**:
`git fetch origin claude/youthful-mayer-qs7e0h && git checkout claude/youthful-mayer-qs7e0h && git pull`.
If your environment names another branch, this one still wins.

**Before you start:** prompt 1 landed. `CHANGELOG.md` mentions `@` mentions
attaching file contents. If it doesn't, stop and report.

## Read first
- `docs/next/README.md`: the shared rules (they apply here in full).
- `docs/ARCHITECTURE.md`: C17, and especially "Immediate commands". That note
  explains today's limit; your work replaces it.
- `docs/TUI.md` ("Session task", the key table), `docs/PARITY.md` (the `/btw`
  row says "Not yet runnable while a turn is busy").
- The code:
  - `crates/forge-core/src/commands/mod.rs`: `BUILTINS`. The commands with
    `immediate: true` are `/btw`, `/context`, `/mcp`, `/status`, `/tasks`,
    `/usage` and the TUI's `/keybindings` and `/terminal-setup`.
  - `crates/forge-core/src/commands/run.rs`: `status(d)`, `usage(d)`,
    `tasks(d, args)`. `crates/forge-core/src/commands/session.rs`:
    `context(d, args)`, `btw(d, args)`. `crates/forge-core/src/commands/mcp.rs`:
    `run`. All of them take `&Driver` or `&mut Driver` today.
  - `crates/forge-core/src/driver.rs`: `Driver::input` holds `&mut self` for
    the whole turn, and `Live` already shares the engine handle across session
    switches.
  - `crates/forge-engine/src/engine.rs`:
    - `record_usage` and the tool-batch loop in `submit`: where state changes
      during a turn;
    - `EngineHandle`: runtime and permissions, already shared.
  - `crates/forge-core/src/subtask.rs` (`Subtasks`) and
    `crates/forge-core/src/schedule_tools.rs` (`SharedScheduler`, an
    `Arc<Mutex<_>>` already).
  - Front ends:
    - `crates/forge-cli/src/tui/session.rs` (`run`: the `select!` loop);
    - `crates/forge-cli/src/tui/app.rs` (`submit` queues while `busy`; it
      already answers `/keybindings` and `/terminal-setup` itself);
    - `crates/forge-cli/src/main.rs` (`run_print`: the stream-json input loop);
    - `crates/forge-cli/src/host.rs` (`read_stdin`, `ControlContext`: the
      reader runs beside the turn).

## Today
A turn holds the driver mutably. The TUI queues `/status` typed during a
turn, and a stream-json host's `/status` waits in the input channel. Both get
the answer only after the turn ends.

## The work

### 1. A read-only session view
Add `forge_core::view::SessionView`, cheap to clone (`Arc` inside), with
everything the immediate commands read:
- session info (id, cwd, title, init facts, settings files, warnings);
- the runtime (model, effort, fast mode, thinking);
- permission mode, rules and working directories;
- cost, usage, per-model usage, context tokens, activity counters;
- the conversation as a shared snapshot (`Arc<Vec<Message>>` swapped, not
  copied, on each update), system blocks and tool specs (for `/context` and
  `/btw`);
- memory files, subtask rows, scheduled tasks, and the MCP manager (an `Arc`
  already).

Updates:
- The engine publishes its part (an `EngineSnapshot` behind
  `Arc<RwLock<Arc<EngineSnapshot>>>`, replaced whole) after each API call
  (`record_usage`), after each tool batch, and when a turn ends.
- The driver publishes the rest when idle and after switches. `Live` is the
  model: it survives `Driver::switch`.
- Readers take a clone of the inner `Arc`, so no guard is held for long and
  never across `.await`.

### 2. One implementation per command
Rewrite these command bodies to take `&SessionView` (plus their arguments):
- `/status`, `/usage`, `/context`;
- `/tasks` (listing and `stop <id>`: give the view the subtasks' stop tokens
  and the shared scheduler);
- `/mcp` (listing, plus reconnect, enable and disable through the manager).

The idle path calls the same functions with a fresh view, so the text output
doesn't change. Existing tests in `driver_tests.rs` and
`crates/forge-cli/tests/commands.rs` must pass unchanged.

Effects that need the driver are recorded on the view and applied by the
driver when the turn ends:
- `/mcp`'s `refresh_mcp` (instructions and `system/init`);
- `/btw`'s cost (`record_side_usage`) and its entry in `side_questions`.

`/btw` mid-turn: answer from the snapshot (same system prompt, tools,
`tool_choice: none`, as today), using the provider in the view.

A new function decides it: `commands::immediate(text, &catalog) -> bool`,
true for an immediate built-in. Aliases count (`/cost`). Arguments may
matter: say whether `/mcp reconnect x` is immediate (it should be, the
manager serializes per server).

### 3. Front ends
- **TUI:**
  - `App::submit`: an immediate command while busy is sent at once
    (`Action::Send`), echoed, and not queued;
  - `session::run` drives `driver.input(..)` as a pinned future inside
    `select!` with `rx.recv()`: immediate inputs are answered from the view,
    and anything else waits in a local queue for the turn to end;
  - the spinner keeps running.
- **Stream-json:** the reader (`host::read_stdin`) recognizes an immediate
  command in a `user` message and answers it at once with a `result` message
  (`num_turns: 0`, the usual shape, the current session id), without sending
  it to the turn loop. Document how a host tells it apart from the running
  turn's result; prefer the reference's shape if its public docs say,
  otherwise add `"immediate": true`. Keep the stream-json golden stable unless
  you change it on purpose.
- **REPL:** the line REPL reads stdin only between turns. Leave it as it is,
  and document that.
- **`-p` with a single prompt:** nothing changes.

### 4. Tests
- A driver-level test: while a slow turn runs (`MockTurn::with_delay`),
  `/status`, `/usage` and `/tasks` answer from the view, and their numbers
  update after the first API call of that turn.
- A TUI session test (`tui::session::tests`): send a slow turn, then
  `/status`. The `Reply` arrives before `Idle`, and a non-immediate input
  still waits.
- A TUI app test: `/usage` typed while busy is sent, not queued.
- `/btw` mid-turn: answers, and its cost is in `/usage` after the turn.
- `/mcp reconnect` mid-turn: the instructions refresh after the turn.
- An end-to-end stream-json test in `crates/forge-cli/tests/e2e.rs`: a slow
  turn, then a `/status` user message. Its result line arrives before the
  turn's result.
- No std lock guard across `.await` (clippy's `await_holding_lock` is a good
  check: add `#![warn(clippy::await_holding_lock)]` to the crates you touch if
  it isn't on).

### 5. Docs
- `docs/ARCHITECTURE.md` "Immediate commands": rewrite it (what the view
  holds, when it updates, deferred effects, per-surface behaviour).
- `docs/TUI.md`: the session task loop and the key table.
- `docs/CLI.md`: the stream-json section.
- `docs/PARITY.md`: `/btw`, `/status`, `/usage`, `/tasks`, `/context`, `/mcp`.
- `CHANGELOG.md`.
- `docs/GOALS.md` priority 7: remove the item from "Still to build".
- Add manual checks to `docs/CHECKLIST.md`: `/usage` during a long real turn
  in the TUI, and `/btw` mid-turn.

## Finish
Gate, commit (one commit per step is fine; `Goals: priority 7, C17`), push,
and report what's done, what isn't and the manual checks you added.
