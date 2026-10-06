//! A line-based interactive session (until the full-screen UI lands).

use std::io::Write;
use std::sync::Arc;

use forge_engine::{EngineEvent, EventSink, NoticeLevel, PermissionAnswer, PermissionPrompt, PermissionPrompter};
use forge_types::{ContentBlock, Delta, MessageContent, StreamEvent};
use tokio::sync::{mpsc, Mutex};

use crate::args::Opts;

type Lines = Arc<Mutex<mpsc::UnboundedReceiver<String>>>;

fn spawn_stdin() -> Lines {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line.trim_end_matches(['\n', '\r']).to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    Arc::new(Mutex::new(rx))
}

struct LinePrompter {
    lines: Lines,
}

#[async_trait::async_trait]
impl PermissionPrompter for LinePrompter {
    async fn ask(&self, p: PermissionPrompt) -> PermissionAnswer {
        let summary = summarize_input(&p.tool_name, &p.input);
        eprint!("\n  Allow {}{}? [y]es / [a]lways / [n]o: ", p.tool_name, summary);
        let _ = std::io::stderr().flush();
        let answer = self.lines.lock().await.recv().await.unwrap_or_default();
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] },
            "a" | "always" => PermissionAnswer::Allow {
                updated_input: None,
                updated_permissions: p.suggestions.iter().filter_map(|s| serde_json::to_value(s).ok()).collect(),
            },
            _ => PermissionAnswer::Deny { message: String::new(), interrupt: false },
        }
    }
}

fn summarize_input(tool: &str, input: &serde_json::Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" | "MultiEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        _ => "",
    };
    match input.get(key).and_then(|v| v.as_str()) {
        Some(v) => format!("({})", v.chars().take(120).collect::<String>()),
        None => String::new(),
    }
}

struct PrintSink;

impl EventSink for PrintSink {
    fn emit(&self, event: EngineEvent) {
        let mut so = std::io::stdout();
        match event {
            EngineEvent::Stream {
                event: StreamEvent::ContentBlockDelta { delta: Delta::TextDelta { text }, .. },
                ..
            } => {
                let _ = write!(so, "{text}");
                let _ = so.flush();
            }
            EngineEvent::Stream { event: StreamEvent::MessageStop, .. } => {
                let _ = writeln!(so);
            }
            EngineEvent::Assistant { message, .. } => {
                for b in &message.content {
                    if let ContentBlock::ToolUse { name, input, .. } = b {
                        let _ = writeln!(so, "● {name}{}", summarize_input(name, input));
                    }
                }
            }
            EngineEvent::User { message, is_meta: false, .. } => {
                for b in &message.content {
                    if let ContentBlock::ToolResult { content, is_error, .. } = b {
                        let text = content.to_text();
                        let first = text.lines().next().unwrap_or("").chars().take(160).collect::<String>();
                        let more = text.lines().count().saturating_sub(1);
                        let mark = if is_error == &Some(true) { "  ⎿ ✗ " } else { "  ⎿ " };
                        let _ = writeln!(
                            so,
                            "{mark}{first}{}",
                            if more > 0 { format!(" (+{more} lines)") } else { String::new() }
                        );
                    }
                }
            }
            EngineEvent::Notice { level, text } => {
                let tag = match level {
                    NoticeLevel::Info => "",
                    NoticeLevel::Warning => "warning: ",
                    NoticeLevel::Error => "error: ",
                };
                eprintln!("{tag}{text}");
            }
            _ => {}
        }
    }
}

pub async fn run(prompt: Option<String>, o: Opts) -> anyhow::Result<i32> {
    let lines = spawn_stdin();
    let lo = crate::launch_options(&o)?;
    let session = forge_core::build_session(lo, Arc::new(PrintSink), Arc::new(LinePrompter { lines: lines.clone() }))?;
    for w in &session.warnings {
        eprintln!("forge: {w}");
    }
    let mut engine = session.engine;
    let handle = engine.handle();
    eprintln!(
        "ForgeCLI {} · {} · {}  (Ctrl-C interrupts a turn; /exit quits)",
        forge_core::VERSION,
        session.init.model,
        session.init.cwd
    );
    // Ctrl-C interrupts the running turn instead of killing the process.
    let h2 = handle.clone();
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            h2.interrupt();
        }
    });
    let mut next = prompt;
    loop {
        let text = match next.take() {
            Some(t) => t,
            None => {
                eprint!("\n> ");
                let _ = std::io::stderr().flush();
                match lines.lock().await.recv().await {
                    Some(l) => l,
                    None => break,
                }
            }
        };
        let t = text.trim();
        if t.is_empty() {
            continue;
        }
        if t == "/exit" || t == "/quit" {
            break;
        }
        let r = engine.submit(MessageContent::Text(t.to_string())).await;
        if let Some(b) = r.prompt_blocked {
            eprintln!("{b}");
        }
    }
    engine.end_session("prompt_input_exit").await;
    Ok(0)
}
