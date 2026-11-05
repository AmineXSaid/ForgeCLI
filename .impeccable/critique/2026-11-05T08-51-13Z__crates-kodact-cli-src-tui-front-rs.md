---
target: KodAct welcome page (frame style)
total_score: 26
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 3
target_identity: "file:/home/user/ForgeCLI/crates/kodact-cli/src/tui/front.rs"
target_fingerprint: "sha256:15771ef01d1746c0ee569ead665d0d062d1ea4fc6505cdcf09f1913d7efbd949"
target_path: /home/user/ForgeCLI/crates/kodact-cli/src/tui/front.rs
timestamp: 2026-11-05T08-51-13Z
slug: crates-kodact-cli-src-tui-front-rs
---
Method: dual-agent (A: design review · B: detector + rendered scan)

## Design Health Score: 26/40 (Acceptable)
| # | Heuristic | Score | Key issue |
|---|---|---|---|
| 1 | Visibility of system status | 3 | Mode, model, endpoint, folder shown; connected servers only shown when they fail |
| 2 | Match system / real world | 3 | "Default" names a list position, not a behaviour; endpoint means nothing to a newcomer |
| 3 | User control and freedom | 3 | /resume offered; no visible exit or help key |
| 4 | Consistency and standards | 2 | Status line right side = version on the front page, keys everywhere else; lime = brand, Default mode and commands |
| 5 | Error prevention | 3 | Mode visible and colour-coded before the first send |
| 6 | Recognition rather than recall | 1 | / @ shift+tab ? hidden on the one screen a newcomer needs them |
| 7 | Flexibility and efficiency | 3 | Keyboard-first; resume needs 7 typed characters; 11 rows of art |
| 8 | Aesthetic and minimalist design | 3 | Restrained, but the 2x wordmark outweighs the task |
| 9 | Error recovery | 3 | "didn't start · /mcp" points at the fix; a long model name silently drops the mode row |
| 10 | Help and documentation | 2 | ? lists every key, but nothing here says ? exists |

## Priority issues
- [P1] Front page hides the key hints (render.rs status(): app.front shows v0.1.0); docs/TUI.md says the hints show. Fix: hints on the front page too.
- [P1] The wordmark outranks the input: 2x art is 106x11 cells, the placeholder is the dimmest text (3.9:1). Fix: 2x only on tall terminals; placeholder in MUTED.
- [P1] Contrast: SUBTLE #767b82 is 4.40 on page / 3.89 on panel (dark); #83878d is 3.31 / 3.00 (light). Detector: 92-108 low-contrast findings, all this pair (placeholder, endpoint, "2h ago", footer path and version).
- [P2] A long model name removes the whole mode row (render.rs panel(): meta row skipped when too wide). Fix: always keep the mode; drop the endpoint, then fit the model.
- [P2] "Default" says nothing; name the behaviour or add the shift+tab hint.

## Persona red flags
Alex: 11 rows of art every launch; /resume typed out, no one-key resume. Jordan: no hints, unexplained "Default"/endpoint, empty below the panel with no history. Sam: placeholder under 4.5:1 in both themes; half-block art read before the prompt; DIM-only placeholder in 16/no colour.

## Minor
Resume line centred while panel text is left-aligned; panel 96 wide under a 106-wide logo; mark only from 136 cols; short tagline drops "on the model you choose"; light shadow #c9dca0 1.35:1 makes the light logo flat; empty below-panel slot on first run.
