You are continuing work on **ForgeCLI**. It is a Rust-native agentic coding
CLI (repo `AmineXSaid/ForgeCLI`) whose behavioural reference is the leading
commercial terminal coding agent. This is prompt **3 of 4** in `docs/next/`.
Work autonomously to the end of the list below. Don't ask me questions; make
reasonable decisions and record them in the docs.

## Branch

Work and push only on **`claude/youthful-mayer-qs7e0h`**:
`git fetch origin claude/youthful-mayer-qs7e0h && git checkout claude/youthful-mayer-qs7e0h && git pull`.
If your environment names another branch, this one still wins.

**Before you start:** prompts 1 and 2 landed. `CHANGELOG.md` mentions `@`
attachments and immediate commands mid-turn, and `forge_core::view::SessionView`
exists. If not, stop and report.

## Read first
- `docs/next/README.md`: the shared rules (they apply here in full).
- `docs/TUI.md`, all of it: the inline-viewport decision, the architecture,
  dialogs, pickers, keys.
- `docs/PARITY.md`. These rows point at this work:
  - `/hooks` ("the editor dialog is T (M8)");
  - `/agents` ("the creation wizard");
  - `/diff` ("the interactive viewer");
  - `/context` ("the colour grid");
  - `/keybindings` ("keys can't be rebound yet");
  - `/color`, `/focus`, `/tui`, `/scroll-speed` (todo).
- The code:
  - `crates/forge-cli/src/tui/app.rs`: `DialogKind` (Permission, Plan,
    Questions, Picker), `dialog_key`, `choose`, `KEYS`, `keys_text`, `on_key`.
  - `crates/forge-cli/src/tui/render.rs`: `dialog`, `boxed`, `live_view_at`
    (height limits: it never drops dialog rows).
  - `crates/forge-cli/src/tui/session.rs`:
    - how pickers are answered (`forge_core::commands::picker::picker`
      before `driver.input`);
    - `ui_command` for UI-only commands.
  - `crates/forge-core/src/commands/picker.rs`: the pattern to copy. forge-core
    builds the data, the TUI draws it, and choices are command text.
  - `crates/forge-core/src/commands/session.rs`: `diff`, `context`.
    `crates/forge-core/src/commands/run.rs`: `hooks`, `agents`.
    `crates/forge-core/src/commands/settings.rs`: `save` and `save_default`,
    which write settings and mirror them into the loaded layers.
  - `crates/forge-agents/src/definitions.rs`: the agent file format
    (frontmatter name, description, tools, model).
  - `crates/forge-hooks/src/lib.rs`: the hooks settings shape
    (`{"hooks": {"<Event>": [{"matcher": "...", "hooks": [{"type": "command", "command": "..."}]}]}}`).

## Design rule
Keep the TUI an **inline viewport** (no alternate screen). A screen is a
dialog in the live region, at most the terminal height minus 1. Long content
scrolls inside the dialog (Up/Down, PageUp/PageDown, Home/End), and Esc
closes it. Every screen has a text form that stays the answer on other
surfaces (`-p`, stream-json, REPL), so their tests don't change.

Pattern for each screen:
1. forge-core gets structured data next to the text command (for example
   `commands::screens::diff(&view) -> DiffScreen`). The text command renders
   from the same data.
2. The TUI session task answers the bare command with
   `UiEvent::Screen(Screen::Diff(..))`, as it does for pickers. Thanks to
   prompt 2 it works mid-turn too, from the `SessionView`.
3. `app.rs` gets `DialogKind::Viewer` (scrolling lines) and a form kind (fields
   plus Enter and Tab) where a screen needs input. `render.rs` draws them.
4. Anything a screen changes runs as command text the person could type (add
   argument forms where missing), exactly like pickers.

## The work, in order

1. **A scrolling viewer dialog** (`DialogKind::Viewer { title, lines, scroll }`),
   with `TestBackend` tests for scrolling and height limits.
2. **`/diff` viewer:**
   - the files changed (from git, or from the session's file history outside
     git), then each file's hunks;
   - additions green, deletions red, hunk headers dim; with no colour, `+`
     and `-` only;
   - Enter on a file jumps to it, Esc goes back.
3. **`/context` grid:** a 10×N block grid coloured by part (system, tools,
   MCP, skills, memory, messages, free), with the legend and numbers from the
   same data as the text `/context`. `/context all` stays text.
4. **`/hooks` editor:**
   - list the events, then each event's matchers and commands, with the
     source layer of each;
   - "Add hook…" asks for the event (a picker), the matcher and the command,
     then the scope (local, project, user);
   - "Remove" deletes one;
   - writes go through the existing settings save helpers;
   - add argument forms first
     (`/hooks add <Event> <matcher> <command> [--scope ..]`,
     `/hooks remove <Event> <n>`) and test them in `driver_tests.rs`; the
     editor then runs them.
5. **`/agents` wizard:**
   - name, description, tools (a multi-select from the tool list), model
     (a picker), scope (project `.forge/agents/` or user);
   - writes `<name>.md` in the format `definitions.rs` reads, then reloads
     (`/reload-skills`);
   - an argument form for the same
     (`/agents create <name> --description .. --tools a,b --model ..`) with
     tests.
6. **Key rebinding:**
   - `<config>/keybindings.json` (`forge_config::config_dir()`) maps key specs
     to named actions (for example `{"ctrl+r": "historySearch", "ctrl+g": "none"}`);
   - define the action names from today's `KEYS` and document them;
   - `app.rs` looks actions up through a table built at start;
   - `KEYS` and `keys_text` show the bindings in effect;
   - bad entries become a warning at start, not a failure;
   - test parsing, overrides and conflicts.
7. **`/color`, `/focus`, `/tui`, `/scroll-speed`:** decide what each means in
   an inline-viewport UI and either build it or mark it "out" in PARITY with
   the reason. Suggested:
   - `/color <name>`: the accent colour for this session;
   - `/focus`: hide tool-call lines in scrollback until it is turned off;
   - `/tui`: out (Forge has one UI);
   - `/scroll-speed`: out (the terminal scrolls).

## Tests
- A `TestBackend` snapshot for each screen: colour and no colour, 40 and 100
  columns.
- App key tests for navigation and closing.
- `driver_tests.rs` tests for each new argument form, with settings files in
  temp dirs. Never write the real user settings: set
  `d.info.user_settings` to a temp path, as the existing tests do.

## Docs
- `docs/TUI.md`: a "Screens" section and a "Phase 4" in "Phases".
- `docs/PARITY.md`: each row named above.
- `docs/CLI.md`: the new argument forms and `keybindings.json`.
- `CHANGELOG.md`.
- `docs/GOALS.md` priority 7: update "Still to build".
- Add manual checks to `docs/CHECKLIST.md` for each screen in a real terminal.

## Finish
Gate, commit (one per screen; `Goals: priority 7`), push, and report what's
done, what isn't and the manual checks you added.
