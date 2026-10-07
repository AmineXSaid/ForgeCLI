# The interactive terminal UI (M8)

`forge` with no `-p` opens the interactive UI when stdin and stdout are both
terminals. `--no-tui` (or `FORGE_TUI=0`) keeps the line REPL
(`crates/forge-cli/src/repl.rs`), and print mode is unchanged.

## Status

| Part | State | Where |
| --- | --- | --- |
| Prompt editor: multiline text, word moves, kill commands, history | done, tested | `crates/forge-cli/src/tui/editor.rs` |
| Theme, one-line markdown, wrapping by display width | done, tested | `crates/forge-cli/src/tui/text.rs` |
| UI state machine: keys, streaming into scrollback, tool lines, `/` menu, permission/question/plan dialogs, queued input, Esc/Ctrl-C/Shift+Tab | done, tested | `crates/forge-cli/src/tui/app.rs` |
| Session task (owns the Driver), `TuiSink`, `TuiPrompter` | to do (phase 1) | `crates/forge-cli/src/tui/session.rs` |
| Renderer for the live region (`LiveView`) | to do (phase 1) | `crates/forge-cli/src/tui/render.rs` |
| Terminal loop, inline viewport, scrollback writes, panic-safe restore | to do (phase 1) | `crates/forge-cli/src/tui/mod.rs` |
| Wiring into `main`, `--no-tui`, `FORGE_TUI`, persisted history | to do (phase 1) | `crates/forge-cli/src/main.rs`, `args.rs` |
| Pickers, Ctrl+R search, `@file` completion | to do (phase 2) | |
| UI-only commands (`/theme`, `/copy`, `/keybindings`, `/statusline`, `/terminal-setup`) | to do (phase 3) | |

Dependencies are in `crates/forge-cli/Cargo.toml`:
- `ratatui` 0.29 (MIT);
- `crossterm` 0.28 with `event-stream` (MIT);
- `unicode-width` 0.2 (MIT or Apache-2.0).

`tui/mod.rs` declares the modules with `#![allow(dead_code)]` until the loop
uses them. Remove that attribute when phase 1 is wired in.

## Decision

The UI is an **inline viewport**, not an alternate screen:
- finished output (your prompts, answers, tool calls, notices) is written
  into the terminal's own scrollback, so scrolling, search, copy and the
  history after exit work as in any shell;
- only a small live region at the bottom is redrawn: the streaming answer's
  unfinished line, the spinner, a dialog when one is open, queued messages,
  the input box, the `/` menu and the status line.

This is how the reference CLI behaves, and it keeps the UI robust: nothing
on screen depends on a virtual scroll buffer, and a crash leaves readable
output behind. The live region grows and shrinks with what it shows.

## Architecture

```
 crossterm EventStream ──► UI task ──(ToSession)──► session task (owns Driver)
                           App      ◄──(UiEvent)───  TuiSink, TuiPrompter, results
                           Screen: live viewport draw + insert_before(scrollback)
```

### Session task (`tui/session.rs`, to do)

```rust
pub enum ToSession { Input(String), Exit }

pub async fn run(mut driver: forge_core::Driver,
                 mut rx: mpsc::UnboundedReceiver<ToSession>,
                 ui: mpsc::UnboundedSender<app::UiEvent>)
```

**The loop** mirrors `repl.rs` (whose prompt loop is the reference):
- At the top of each idle pass:
  - `let _ = *finished.borrow_and_update();` (where `finished = driver.subtasks.watch()`);
  - `driver.deliver_subtasks();`
  - send `UiEvent::Status(..)` and `UiEvent::Idle`.
- Then `select!` on:
  - `rx.recv()`:
    - `Input(text)` runs `driver.input(MessageContent::Text(text), &mut report).await`;
    - if that returns `Flow::Exit`, send `UiEvent::Exit` and break;
    - `Exit` or a closed channel breaks.
  - `sleep(driver.next_wait())` when it is `Some`: run `driver.run_due(&mut report)` if `driver.task_due()`.
    A due task makes the session busy too, so the UI gets no `Idle` until it ends.
  - `finished.changed()`: continue (the next idle pass hands the subtask back).
- After the loop, `driver.shutdown("prompt_input_exit").await`.

**The `report` callback** turns each `TurnResult` into what the UI shows:
- a local command result (`num_turns == 0 && stop_reason.is_none()`) becomes
  `UiEvent::Reply { text: result, is_error }`;
- a blocked prompt (`prompt_blocked`) becomes `Reply { is_error: true }`;
- a model turn that errored becomes `Reply { text: errors or result, is_error: true }`;
- a successful model turn sends nothing: its text already streamed.

**The status snapshot** after each input (`app::StatusView`):
- model: `driver.handle().model()`;
- mode: `driver.handle().permissions.read().unwrap().mode.as_str()`;
- cwd: `driver.info.cwd`;
- cost: `driver.engine.state.total_cost_usd`;
- context %: `driver.engine.state.context_tokens` over the model's window
  (`forge_api::models::model_info(model).context_window`).

Also send `UiEvent::Commands` from `driver.catalog.catalog_json(Surface::Tui)`
(fields `name`, `argumentHint`, `description`). Send it once at start and
again whenever `driver.info.session_id` changes (`/clear`, `/resume`,
`/branch`, `/cd` and the reloads change commands).

**`TuiSink`** (an `EventSink`) sends every `EngineEvent` as
`UiEvent::Engine(e)`. The channel is unbounded, so it never blocks the engine.

**`TuiPrompter`** (a `PermissionPrompter`) creates a `oneshot`, sends
`UiEvent::Ask { prompt, reply }` and awaits the answer. If the receiver is
dropped (UI gone), it answers
`Deny { message: "The UI closed.", interrupt: true }`.

**Building the session** copies `repl::run`:
- `launch_options`;
- `connect_mcp`;
- `build_session(lo, sink, prompter)`;
- `Driver::new(session, Surface::Tui, Some(mcp))`;
- `driver.set_rebuild(..)`;
- take `let live = driver.live();` **before** moving the driver into its task.
  The UI uses `live` for interrupts and modes.

### UI state (`tui/app.rs`, done)

Public surface the loop uses:
- `App::new(theme, history)`;
- `app.on_term_event(crossterm::event::Event) -> Vec<Action>`;
- `app.on_event(UiEvent)`;
- `app.take_pending() -> Vec<Line>`: lines for scrollback, already styled
  but not wrapped;
- `app.next_queued() -> Option<String>`: call after every event; send what it
  returns;
- `app.close_dialog_if_stale()`: call after an interrupt;
- `app.exit` and `app.dirty`;
- read-only state the renderer draws from:
  - `editor`, `live` (the unfinished answer line);
  - `busy`, `busy_since`, `activity`, `queued`;
  - `dialog_text()`, `dialog_options()`, `dialog_selected()`;
  - `menu()`, `menu_selected`;
  - `status`, `hint`.

`Action` and how the loop applies it:

| Action | The loop does |
| --- | --- |
| `Send(text)` | `to_session.send(ToSession::Input(text))` (the app already echoed it and marked itself busy) |
| `Interrupt` | `live.handle().interrupt()`, then `app.close_dialog_if_stale()` |
| `SetMode(m)` | `live.handle().set_permission_mode(m)` |
| `Exit` | `to_session.send(ToSession::Exit)`, then wait (at most 5 s) for the session task to end |

### Renderer (`tui/render.rs`, to do)

```rust
pub struct LiveView { pub lines: Vec<Line<'static>>, pub cursor: Option<(u16, u16)> }
pub fn live_view(app: &App, width: u16, max_height: u16) -> LiveView
```

The live region, top to bottom (each part only when present):
1. `app.live`, the unfinished answer line, rendered with
   `text::Markdown::line` and wrapped. At most 3 rows, the last ones.
2. The spinner while `busy`:
   - frames `· ✢ ✳ ✶ ✻ ✽`, advanced every 120 ms from `busy_since`;
   - then `{activity}… ({secs}s · esc to interrupt)`, in the accent style.
3. A dialog: a rounded box (`Block::bordered().border_type(Rounded)`) with:
   - the title (`dialog_text().0`) and body lines;
   - the numbered options from `dialog_options()`, the selected one marked
     `❯` and styled `theme.selected()`, descriptions dimmed on the right;
   - a footer `Enter to select · Esc to cancel`.
4. Queued messages: `  ⏎ {text}` (dim).
5. The input box:
   - a dim `─` rule above and below;
   - rows prefixed `> ` (first) or two spaces;
   - wrapped by character to `width - 2` so the cursor maths is exact;
   - the cursor goes at the editor position (`editor.position()` mapped
     through the wrap);
   - an empty, idle prompt shows a dim placeholder: `Try "explain this repo" · / for commands`.
6. The `/` menu: up to 8 rows of `/name args` padded to the longest name,
   then the description (dim). The selected row uses `theme.selected()`.
7. The status line:
   - left: the hint if one is fresh (under 2 s), else the mode:
     - `⏵⏵ accept edits on (shift+tab to cycle)`;
     - `⏸ plan mode on (shift+tab to cycle)`;
     - `? for shortcuts` for the default mode;
   - right: `{model} · {ctx}% context · ${cost:.2}`.

If the lines exceed `max_height` (the terminal height minus 1), drop rows
from part 1, then part 4, never from the dialog or the input.

### Terminal (`tui/mod.rs`, to do)

```rust
pub async fn run(prompt: Option<String>, o: Opts) -> Result<i32, Fail>
```

**Set up:**
- `enable_raw_mode`, `EnableBracketedPaste`;
- if `crossterm::terminal::supports_keyboard_enhancement()`, also
  `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)`, so Shift+Enter
  is a distinct key;
- a panic hook that restores the terminal (pop flags, disable paste, disable
  raw mode, show the cursor) and then calls the previous hook;
- restore the same way on every exit path, through a guard whose `Drop`
  restores.

**`Screen`** wraps `Terminal<CrosstermBackend<Stdout>>` with
`Viewport::Inline(h)`:
- `set_height(h)`: when `h` differs, call `terminal.clear()` (the cursor
  goes to the viewport's top and everything below is cleared), drop the
  terminal, and create a new one with `Viewport::Inline(h)`. ratatui 0.29
  can't change an inline viewport's height in place. Grow at once; shrink
  only when idle or when a dialog closes, to avoid flicker while streaming.
- `commit(lines)`: wrap with `text::wrap(lines, width)` and write them with
  `terminal.insert_before(n, |buf| Paragraph::new(lines).render(buf.area, buf))`
  in chunks of at most 100 rows.
- `draw(&LiveView)`: render the lines with a `Paragraph` into the frame and
  set the cursor.

**The loop**, with frames drawn at most every 16 ms:

```text
loop {
  if !app.pending.is_empty() { screen.commit(app.take_pending()) }
  if app.dirty (or the spinner is due): view = render::live_view(..); screen.set_height(view.lines.len()); screen.draw(&view)
  if app.exit { break }
  while let Some(t) = app.next_queued() { to_session.send(Input(t)) }
  select! {
    ev = events.next()         => for a in app.on_term_event(ev) { apply(a) }
    ev = ui_rx.recv()          => app.on_event(ev)   // None: the session ended, exit
    _  = tick(120ms), if app.busy || a hint is showing => app.dirty = true
  }
}
```

Other events:
- `Event::Resize` marks the frame dirty; `terminal.autoresize` runs inside
  `draw`.
- Ctrl+L: `terminal.clear()` and a full redraw.

**On exit:**
- clear the live region;
- restore the terminal;
- print `Resume this conversation with: forge --resume <id>`;
- save the history.

**Prompt history** is
`forge_config::state_dir().join("history.jsonl")`, one JSON line per prompt
with `{"text", "cwd"}`:
- load the last 500 entries for this project directory into
  `App::new(.., history)`;
- append each sent prompt;
- the file is mode 0600.

**Choosing the UI** (`main.rs`):
- open the TUI when stdin and stdout are both terminals and neither
  `--no-tui` nor `FORGE_TUI=0|false|off` is set;
- otherwise use `repl::run`.

`term::Term` has `stdin_tty`. Add a `stdout_tty` field
(`std::io::stdout().is_terminal()`). Add `--no-tui` to `args.rs` and
`docs/CLI.md`, and pass `term::get().color` to `Theme { color }`.

## Rendering rules (implemented in `app.rs` and `text.rs`)

- **Assistant text** streams into `app.live`. Each complete line moves to
  scrollback. The first line of a text block gets `⏺ `, the others two
  spaces.
- **Markdown**, one line at a time:
  - headings and `**bold**` are bold;
  - `` `code` `` uses the code style;
  - fence lines are dim, and fenced lines are indented and coloured as code.
- **Tool calls**: `⏺ Name(main argument)` (`app::summarize`). Their results:
  `  ⎿  first line (+N lines)`, red on error.
- Events of sub-agents (those with a `parent_tool_use_id`) are not shown.
- **Your prompts**: `> text`, in the user style.
- **Local command output**: `  ⎿  ` on the first line, then indented.
- **Notices**: dim, yellow (warning) or red (error).
- **System events**: `compact_boundary` shows "Conversation compacted.", and
  `model_fallback` a warning.
- **Colour**: `Theme { color: false }` (`NO_COLOR`, `--color never`) uses
  only bold, dim and reverse.

## Keys (implemented in `app.rs`)

| Key | Does |
| --- | --- |
| Enter | Send (queued while a turn runs); in the `/` menu, run the command, or complete it when it takes arguments |
| Shift+Enter, Alt+Enter, Ctrl+J, `\` then Enter | New line |
| Esc | Interrupt the turn; close the menu; cancel a dialog (its last option); twice on an empty prompt: `/rewind` |
| Ctrl+C | Clear the input; interrupt the turn (and deny an open dialog); twice on an empty prompt: exit |
| Ctrl+D | Exit (empty prompt) |
| Shift+Tab | Next permission mode: default, acceptEdits, plan |
| Up / Down | Line up/down; history on the first/last line; menu or dialog selection |
| Tab | Complete the highlighted `/` command |
| Ctrl+A / Ctrl+E, Home / End | Start / end of line |
| Ctrl+W, Alt+Backspace | Delete the word before the cursor |
| Ctrl+U / Ctrl+K | Delete to the start / end of the line |
| Alt+B / Alt+F, Ctrl+Left / Ctrl+Right | Word left / right |
| 1-9 in a dialog | Choose that option |
| Space in a multi-select question | Toggle the option |
| Ctrl+L | Redraw (loop, to do) |

Pasted text (bracketed paste) is inserted as typed, newlines included.

## Dialogs (implemented in `app.rs`)

| Dialog | Options | Answer |
| --- | --- | --- |
| Permission | 1. Yes | `Allow` |
| | 2. Yes, and don't ask again for `<rule>` (only when the prompt has suggestions) | `Allow` with the suggestions as `updated_permissions` |
| | 3. No, and tell Forge what to do instead (Esc) | `Deny { interrupt: true }` |
| AskUserQuestion | The options, then "Type an answer"; multi-select toggles with Space | `Allow` with `input.answers = {question: answer}` (the shape `LinePrompter` uses) |
| Plan approval (the plan goes to scrollback first) | 1. Yes, and accept edits without asking | `Allow` plus `setMode acceptEdits` (session) |
| | 2. Yes, and ask before each edit | `Allow` |
| | 3. No, keep planning (Esc) | `Deny { interrupt: true }` |

## Testing

- **Unit tests, done (9):** `tui::editor`, `tui::text` and `tui::app`.
  Run them with `cargo test -p forge-cli --bin forge tui`.
- **Renderer (to do):** `ratatui::backend::TestBackend` snapshot tests of
  `live_view` drawn into a fixed area:
  - idle;
  - busy with a spinner and queued input;
  - the `/` menu;
  - each dialog;
  - a narrow width (20 columns);
  - `Theme { color: false }`.
  Compare `buffer` text rows; colours are checked through a few cells.
- **Session (to do):** build a session with `MockProvider` (as
  `crates/forge-core/src/driver_tests.rs` does in `driver_with`), run
  `session::run` on a task, and drive it through the channels:
  - a text turn ends with `Idle`, and `Engine` events carry the text;
  - a `/status` reply arrives as `Reply`;
  - a permission prompt arrives as `Ask`, and the oneshot answer reaches the
    tool;
  - a queued input is sent after `Idle`;
  - `/exit` gives `Exit`.
- **Terminal (to do):** keep `mod.rs` thin. Test `Screen::commit`'s chunking
  and wrapping through a `TestBackend` with `Viewport::Inline`; `insert_before`
  works on it. Check by hand in a real terminal (the checklist below).

## Phases

1. **The session view** (finish it):
   - `session.rs`, `render.rs` and `mod.rs` as above;
   - wiring into `main` with `--no-tui` and `FORGE_TUI`;
   - persisted history;
   - tests;
   - docs:
     - `docs/CLI.md`: the interactive section and `--no-tui`;
     - `docs/PARITY.md`: TUI rows;
     - `CHANGELOG.md`;
     - `docs/CHECKLIST.md`: the manual steps below.
2. **Pickers.** A `Picker` dialog (title, rows, filter by typing). It is
   filled from data the session sends on request
   (`ToSession::Picker(kind)` → `UiEvent::Picker { kind, rows }`):
   - `/model` with no argument: `forge_api::models::MODELS`;
   - `/resume`: `forge_session::SessionStore::list(cwd)`;
   - `/rewind`: `driver.engine.prompt_points()`, then a second step for the
     action (both, conversation, code, summarize from, summarize to);
   - `/output-style`: `driver.catalog.styles`;
   - `/permissions`: the rules, with add and remove.

   Choosing a row sends the existing argument form (`/model opus`,
   `/resume 3`, `/rewind 2 code`, ...), so the commands don't change. Also in
   this phase:
   - Ctrl+R reverse history search in the input box;
   - `@` file completion (a menu of paths from the `ignore` crate's walk of
     the project, filtered as you type).
3. **UI-only commands.**
   - `/theme`: dark, light, no colour; saved as `theme` in user settings.
   - `/copy [N]`: the Nth latest answer to the clipboard via OSC 52.
   - `/keybindings`: shows the key table.
   - `/statusline`: runs the `statusLine.command` setting with the session
     as JSON on stdin (model, cwd, session id, cost, context) and shows its
     first output line as the status line.
   - `/terminal-setup`: explains Shift+Enter for terminals without the
     keyboard protocol.

   Register them in `BUILTINS` with `Surfaces` set to the TUI only, so
   `/help` in other modes leaves them out.

## Manual checks (add to docs/CHECKLIST.md when phase 1 lands)

1. `forge` in a real terminal: the prompt box and the status line appear at
   the bottom. `hello` streams an answer, and scrolling up with the mouse or
   Shift+PageUp shows the whole conversation.
2. During a long answer, press Esc. The turn stops and
   `[Request interrupted by user]` is in the transcript (`/export`).
3. Type a second message while a turn runs. It shows as queued and is sent
   when the turn ends.
4. Ask for a shell command in default mode. The permission dialog appears;
   option 2 adds the rule (`/permissions` lists it).
5. Shift+Tab twice shows plan mode. Asking for a change ends with the plan
   dialog, and option 1 switches to accept-edits.
6. `NO_COLOR=1 forge` uses no colours. A 40-column terminal wraps without
   breaking the box.
7. Leave with `/exit`, with Ctrl-D and with Ctrl-C twice. Each time the
   shell works normally afterwards: typed text echoes, and `stty -a` shows
   `icanon echo`.
