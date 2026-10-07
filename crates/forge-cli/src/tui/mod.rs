//! The interactive terminal UI (M8). The design is in docs/TUI.md.
//!
//! An inline viewport at the bottom of the terminal shows what is live (the
//! answer being written, the spinner, a dialog, the input box, the status
//! line). Finished output goes into the terminal's own scrollback.
//!
//! - `editor`: the prompt's text editor;
//! - `text`: theme, one-line markdown, wrapping;
//! - `app`: the UI state machine (keys and session events);
//! - `render`: the live region, drawn from the app;
//! - `session`: the task that owns the driver;
//! - this module: the terminal, the event loop and prompt history.

pub mod app;
pub mod editor;
pub mod render;
pub mod session;
pub mod text;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use futures::StreamExt;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;

use app::{Action, App, UiEvent};
use render::LiveView;
use session::ToSession;

use crate::args::Opts;
use crate::exit::Fail;

/// Rows written into scrollback per `insert_before` call.
const COMMIT_CHUNK: usize = 100;
/// The fastest the live region is redrawn.
const FRAME: Duration = Duration::from_millis(16);
/// Prompts kept from history for this directory.
const HISTORY_ENTRIES: usize = 500;

/// The live viewport and the scrollback above it.
pub struct Screen<B: Backend> {
    terminal: Terminal<B>,
    /// Makes a fresh backend when the viewport changes height.
    make: Box<dyn FnMut() -> B>,
    height: u16,
}

impl<B: Backend> Screen<B> {
    pub fn new(mut make: Box<dyn FnMut() -> B>, height: u16) -> std::io::Result<Self> {
        let terminal = Terminal::with_options(make(), TerminalOptions { viewport: Viewport::Inline(height) })?;
        Ok(Screen { terminal, make, height })
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn width(&self) -> u16 {
        self.terminal.size().map(|s| s.width).unwrap_or(80)
    }

    pub fn rows(&self) -> u16 {
        self.terminal.size().map(|s| s.height).unwrap_or(24)
    }

    /// Change the viewport's height. ratatui can't resize an inline viewport
    /// in place: clear it (the cursor goes to its top) and make a new one there.
    pub fn set_height(&mut self, h: u16) -> std::io::Result<()> {
        let h = h.max(1);
        if h == self.height {
            return Ok(());
        }
        self.terminal.clear()?;
        self.terminal = Terminal::with_options((self.make)(), TerminalOptions { viewport: Viewport::Inline(h) })?;
        self.height = h;
        Ok(())
    }

    /// Write finished lines into scrollback, above the viewport.
    pub fn commit(&mut self, lines: Vec<Line<'static>>) -> std::io::Result<()> {
        let rows = text::wrap(lines, self.width());
        for chunk in rows.chunks(COMMIT_CHUNK) {
            let chunk = chunk.to_vec();
            self.terminal.insert_before(chunk.len() as u16, |buf| Paragraph::new(chunk).render(buf.area, buf))?;
        }
        Ok(())
    }

    /// Draw the live region. A view shorter than the viewport sits at its bottom.
    pub fn draw(&mut self, view: &LiveView) -> std::io::Result<()> {
        let pad = (self.height as usize).saturating_sub(view.lines.len());
        let mut lines = vec![Line::default(); pad];
        lines.extend(view.lines.iter().cloned());
        self.terminal.draw(|f| {
            let area = f.area();
            f.render_widget(Paragraph::new(lines), area);
            if let Some((x, y)) = view.cursor {
                let y = y + pad as u16;
                if x < area.width && y < area.height {
                    f.set_cursor_position((area.x + x, area.y + y));
                }
            }
        })?;
        Ok(())
    }

    /// Clear the live region (Ctrl+L redraws after this).
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.terminal.clear()
    }

    #[cfg(test)]
    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }
}

// ---- the terminal's modes ----

static ENHANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Put the terminal back as it was: keyboard flags, paste, raw mode, cursor.
fn restore() {
    let mut out = std::io::stdout();
    if ENHANCED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = crossterm::execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = crossterm::execute!(out, DisableBracketedPaste, crossterm::cursor::Show);
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = out.flush();
}

/// Restores the terminal when dropped, on every exit path.
struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

fn setup() -> std::io::Result<Guard> {
    crossterm::terminal::enable_raw_mode()?;
    let guard = Guard;
    let mut out = std::io::stdout();
    crossterm::execute!(out, EnableBracketedPaste)?;
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        // Shift+Enter becomes a key of its own.
        crossterm::execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        ENHANCED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
    }));
    Ok(guard)
}

// ---- prompt history ----

pub fn history_path() -> PathBuf {
    forge_config::state_dir().join("history.jsonl")
}

/// The last prompts typed in `cwd`, oldest first.
pub fn load_history(path: &Path, cwd: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let mut out: Vec<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["cwd"] == cwd)
        .filter_map(|v| v["text"].as_str().map(str::to_string))
        .collect();
    if out.len() > HISTORY_ENTRIES {
        out.drain(..out.len() - HISTORY_ENTRIES);
    }
    out
}

/// Add a prompt to the history file (created with mode 0600).
pub fn append_history(path: &Path, cwd: &str, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    writeln!(f, "{}", serde_json::json!({"text": text, "cwd": cwd}))
}

// ---- `@` completion ----

/// Most paths offered for `@` completion.
const MAX_FILES: usize = 20_000;

/// The project's files and directories (relative, `/`-separated; directories end in `/`),
/// skipping what `.gitignore` and hidden-file rules skip.
pub fn project_files(root: &Path) -> Vec<String> {
    let mut out = vec![];
    for e in ignore::WalkBuilder::new(root).build().flatten() {
        let Ok(rel) = e.path().strip_prefix(root) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut p = rel.to_string_lossy().replace('\\', "/");
        if e.file_type().is_some_and(|t| t.is_dir()) {
            p.push('/');
        }
        out.push(p);
        if out.len() >= MAX_FILES {
            break;
        }
    }
    out
}

fn walk_files(root: String, ui: mpsc::UnboundedSender<UiEvent>) {
    tokio::task::spawn_blocking(move || {
        let _ = ui.send(UiEvent::Files(project_files(Path::new(&root))));
    });
}

// ---- the loop ----

/// The interactive UI: until `/exit`, Ctrl-D or Ctrl-C twice.
pub async fn run(prompt: Option<String>, o: Opts) -> Result<i32, Fail> {
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    let (driver, warnings) = session::build(&o, ui_tx.clone()).await?;
    let live = driver.live();
    let cwd = driver.info.cwd.display().to_string();
    let model = driver.handle().model();
    let persist = !o.no_session_persistence;
    let hist_path = history_path();
    let theme = text::Theme { color: crate::term::get().color };
    let mut app = App::new(theme, load_history(&hist_path, &cwd));
    app.pending.push(Line::from(vec![
        ratatui::text::Span::styled("✻ ", theme.accent()),
        ratatui::text::Span::styled(format!("ForgeCLI {}", forge_core::VERSION), theme.bold()),
        ratatui::text::Span::styled(format!("  {model} · {cwd}"), theme.dim()),
    ]));
    for w in warnings {
        app.on_event(UiEvent::Engine(forge_engine::EngineEvent::Notice {
            level: forge_engine::NoticeLevel::Warning,
            text: w,
        }));
    }

    let (to_session, rx) = mpsc::unbounded_channel::<ToSession>();
    walk_files(cwd.clone(), ui_tx.clone());
    // Re-walked when the session moves (/cd). A weak sender, so the channel still
    // closes when the session task ends.
    let files_tx = ui_tx.downgrade();
    let mut files_root = cwd.clone();
    // The session task holds the other senders (here and in its sink and prompter): when it
    // ends, the UI sees the channel close.
    let task = tokio::spawn(session::run(driver, rx, ui_tx));
    let guard = setup().map_err(|e| Fail::config(format!("cannot set up the terminal: {e}")))?;
    let mut screen = Screen::new(Box::new(|| CrosstermBackend::new(std::io::stdout())), 1)
        .map_err(|e| Fail::config(format!("cannot draw on the terminal: {e}")))?;

    let send = |text: String, to: &mpsc::UnboundedSender<ToSession>| {
        let _ = append_history(&hist_path, &cwd, &text);
        let _ = to.send(ToSession::Input(text));
    };
    if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
        for a in app.submit(p) {
            if let Action::Send(t) = a {
                send(t, &to_session);
            }
        }
    }

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(render::SPINNER_STEP);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_draw = Instant::now() - FRAME;
    let mut had_dialog = false;
    let mut exiting = false;
    loop {
        if !app.pending.is_empty() {
            screen.commit(app.take_pending()).map_err(io_fail)?;
            app.dirty = true;
        }
        if app.exit {
            break;
        }
        while let Some(t) = app.next_queued() {
            send(t, &to_session);
        }
        if !app.status.cwd.is_empty() && app.status.cwd != files_root {
            files_root = app.status.cwd.clone();
            if let Some(tx) = files_tx.upgrade() {
                walk_files(files_root.clone(), tx);
            }
        }
        let now = Instant::now();
        if app.dirty && now.duration_since(last_draw) >= FRAME {
            let max = screen.rows().saturating_sub(1).max(1);
            let view = render::live_view(&app, screen.width(), max);
            let want = (view.lines.len() as u16).min(max);
            let dialog_closed = had_dialog && app.dialog.is_none();
            if want > screen.height() || (want < screen.height() && (!app.busy || dialog_closed)) {
                screen.set_height(want).map_err(io_fail)?;
            }
            screen.draw(&view).map_err(io_fail)?;
            had_dialog = app.dialog.is_some();
            app.dirty = false;
            last_draw = now;
        }
        let hint = app.hint.as_ref().is_some_and(|(_, at)| at.elapsed() < render::HINT_FOR + render::SPINNER_STEP);
        let frame_wait = (last_draw + FRAME).saturating_duration_since(Instant::now());
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release
                    && k.code == KeyCode::Char('l') && k.modifiers.contains(KeyModifiers::CONTROL) => {
                    screen.clear().map_err(io_fail)?;
                    app.dirty = true;
                }
                Some(Ok(ev)) => {
                    for a in app.on_term_event(ev) {
                        match a {
                            Action::Send(t) => send(t, &to_session),
                            Action::Picker(t) => {
                                let _ = to_session.send(ToSession::Picker(t));
                            }
                            Action::Interrupt => {
                                live.handle().interrupt();
                                app.close_dialog_if_stale();
                            }
                            Action::SetMode(m) => live.handle().set_permission_mode(m),
                            Action::Exit => {
                                let _ = to_session.send(ToSession::Exit);
                                exiting = true;
                            }
                        }
                    }
                    if exiting {
                        break;
                    }
                }
                // The terminal is gone.
                Some(Err(_)) | None => break,
            },
            ev = ui_rx.recv() => match ev {
                Some(ev) => {
                    app.on_event(ev);
                    // Take what else is ready, so a burst of deltas is one frame.
                    while let Ok(ev) = ui_rx.try_recv() {
                        app.on_event(ev);
                    }
                }
                None => break,
            },
            _ = tick.tick(), if app.busy || hint => app.dirty = true,
            _ = tokio::time::sleep(frame_wait), if app.dirty && !frame_wait.is_zero() => {}
        }
    }
    // Whatever is left goes to scrollback, then the live region is cleared.
    let _ = screen.commit(app.take_pending());
    let _ = screen.set_height(1);
    let _ = screen.clear();
    drop(screen);
    drop(guard);
    let _ = to_session.send(ToSession::Exit);
    drop(to_session);
    // The session ends its own way (hooks, MCP); don't wait for it forever.
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    if persist {
        eprintln!("Resume this conversation with: forge --resume {}", live.session_id());
    }
    Ok(crate::exit::OK)
}

fn io_fail(e: std::io::Error) -> Fail {
    Fail::config(format!("terminal error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn rows(b: &TestBackend) -> Vec<String> {
        let buf = b.buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().into()
            })
            .collect()
    }

    #[test]
    fn commit_wraps_into_scrollback_above_the_viewport() {
        let mut s = Screen::new(Box::new(|| TestBackend::new(10, 6)), 2).unwrap();
        let view = LiveView { lines: vec![Line::from("> input")], cursor: Some((2, 0)) };
        s.draw(&view).unwrap();
        s.commit(vec![Line::from("the quick brown fox"), Line::from("end")]).unwrap();
        // The loop redraws the live region after each commit.
        s.draw(&view).unwrap();
        let r = rows(s.backend());
        // Committed rows are wrapped to the width and sit above the live region.
        let at = r.iter().position(|l| l == "the quick").expect("committed");
        assert_eq!(r[at + 1], "brown fox");
        assert_eq!(r[at + 2], "end");
        assert!(r[at + 3..].iter().any(|l| l == "> input"), "{r:?}");
    }

    #[test]
    fn commit_writes_long_output_in_chunks() {
        let mut s = Screen::new(Box::new(|| TestBackend::new(20, 8)), 1).unwrap();
        let lines: Vec<Line<'static>> = (0..250).map(|i| Line::from(format!("row {i}"))).collect();
        s.commit(lines).unwrap();
        let r = rows(s.backend());
        // The screen keeps the newest rows just above the viewport.
        assert!(r.iter().any(|l| l == "row 249"), "{r:?}");
        assert!(!r.iter().any(|l| l == "row 0"));
    }

    #[test]
    fn history_is_per_directory_and_private() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state/history.jsonl");
        append_history(&p, "/a", "first").unwrap();
        append_history(&p, "/b", "elsewhere").unwrap();
        append_history(&p, "/a", "multi\nline").unwrap();
        assert_eq!(load_history(&p, "/a"), ["first", "multi\nline"]);
        assert!(load_history(&d.path().join("none"), "/a").is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn project_files_follow_ignore_rules() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::create_dir_all(d.path().join("target")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        std::fs::write(d.path().join("target/out"), "").unwrap();
        std::fs::write(d.path().join(".gitignore"), "target/\n").unwrap();
        std::process::Command::new("git").arg("init").arg("-q").current_dir(d.path()).status().ok();
        let mut files = project_files(d.path());
        files.sort();
        assert_eq!(files, ["src/", "src/main.rs"]);
    }
}
