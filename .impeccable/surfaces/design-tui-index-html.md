---
version: 1
slug: "design-tui-index-html"
primary_target: "design/tui/index.html"
related_targets: []
---

## Scope

Forge's whole terminal UI, as an interactive HTML mock that stays terminal-true (character grid, ANSI colour, Unicode glyphs). Mode: Operate.

## Audience and task

The user's engineering team, all day, on company model endpoints: direct the agent, approve its actions, read answers and changes, return to a session after a break. Must never be showy or slow, hard to scan, or unclear about state.

## Constraints

Keep the Forge name, the pixel F, the orange accent, every command and shortcut. Not the look of the reference CLI or the other popular terminal coding agents. Every element drawable by the real TUI, with 256-colour, 16-colour and NO_COLOR fallbacks.

## Direction contract

THESIS: The session is a working log, written in order and never erased. One fixed margin rail (time, mark, actor, entry number) makes state and trail read down a single column. It refuses chat-in-a-box: no bubbles, no boxed prompt, no rounded panels.

OWN-WORLD: Forge draws on the terminal's own ground: graphite in dark terminals, paper in light ones. Warm ink text, hairline rules, tabular columns. One orange owns live state and attention, and nothing else. Marks form an importance ramp (· read, • change, ✗ failed, ● needs you, ✎ you). Rewound entries stay, struck through. Approvals are signed inline.

STORY: At a glance the engineer sees what is live and whether Forge needs them, approves with the full arguments in view, filters the log by mark, jumps to any entry by number, and reads answers whose lists, code and quotes are set properly.

FIRST VIEWPORT: Header line: pixel mark, project, branch, model, clock. Log column with the margin rail; the live entry's rail cell in orange at the bottom of the log; approval card inline under it with keys. Prompt line at the foot; status line below it: state word, context, cost, edits, background jobs.

FORM: Logbook, candidate 3 of 7 on the ordered list; seed:fe0725774449b6c4. Raises: one column rhythm for every turn (from the folio's fixed scale); colour only for live state (from the tiles' single live accent); every entry addressable by number (from the catalog's codes); each fact shown once (from the sleeve); marks as an importance ramp (from the atlas's magnitude ramp).

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
