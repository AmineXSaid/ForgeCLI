//! The session driver: what every front end (print mode, stream-json, the line
//! REPL, the TUI) runs inputs through. It owns the engine and the command
//! catalog. A message is either a slash command, handled by
//! [`crate::commands::execute`], or a turn for the model.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_config::LoadedSettings;
use forge_engine::{Engine, EngineHandle, TurnResult};
use forge_types::sdk::InitInfo;
use forge_types::MessageContent;

use crate::commands::{self, Catalog, Surface};
use crate::{PromptSpec, Session};

/// Facts about the session for `/status`, `/doctor` and friends.
pub struct SessionInfo {
    pub session_id: String,
    pub init: InitInfo,
    pub settings: LoadedSettings,
    pub warnings: Vec<String>,
    pub cwd: PathBuf,
    /// The user settings file commands save defaults to.
    pub user_settings: PathBuf,
}

/// Activity totals across this process's inputs (for `/usage`).
#[derive(Debug, Default, Clone)]
pub struct Activity {
    pub turns: u32,
    pub api_time: Duration,
    pub prompts: u32,
}

pub struct Driver {
    pub engine: Engine,
    pub catalog: Catalog,
    pub info: SessionInfo,
    pub prompt: PromptSpec,
    pub surface: Surface,
    pub started: Instant,
    pub activity: Activity,
}

/// What one input produced.
#[derive(Debug)]
pub enum Outcome {
    /// A turn's result, or a local command's answer (`num_turns == 0`, no stop reason).
    Result(Box<TurnResult>),
    /// `/exit`: end the session.
    Exit,
}

impl Driver {
    pub fn new(s: Session, surface: Surface, mcp: Option<Arc<forge_mcp::McpManager>>) -> Self {
        let cwd = s.engine.tool_ctx().project_dir.clone();
        Driver {
            engine: s.engine,
            catalog: Catalog {
                commands: s.commands,
                skills: s.skills,
                styles: s.styles,
                plugins: s.plugins,
                agents: s.agents,
                mcp,
            },
            info: SessionInfo {
                session_id: s.session_id,
                init: s.init,
                settings: s.settings,
                warnings: s.warnings,
                cwd,
                user_settings: forge_config::forge_home().join("settings.json"),
            },
            prompt: s.prompt,
            surface,
            started: Instant::now(),
            activity: Activity::default(),
        }
    }

    pub fn handle(&self) -> EngineHandle {
        self.engine.handle()
    }

    /// Run one input: a slash command, or a turn for the model.
    pub async fn input(&mut self, content: MessageContent) -> Outcome {
        let result = match commands::command_text(&content) {
            None => self.engine.submit(content).await,
            Some(text) => match commands::execute(self, &text).await {
                commands::Exec::Submit(prompt) => self.engine.submit(prompt).await,
                commands::Exec::Local { text, is_error } => self.engine.local_result(text, is_error),
                commands::Exec::Exit => return Outcome::Exit,
            },
        };
        if result.num_turns > 0 {
            self.activity.prompts += 1;
            self.activity.turns += result.num_turns;
            self.activity.api_time += Duration::from_millis(result.duration_api_ms);
        }
        Outcome::Result(Box::new(result))
    }

    /// Rebuild the system prompt from [`Driver::prompt`] after changing it.
    pub fn rebuild_system(&mut self) {
        self.engine.set_system(self.prompt.build().0);
    }

    /// An SDK host's `initialize` `systemPrompt` / `appendSystemPrompt`.
    pub fn set_system_prompt(&mut self, replace: Option<String>, append: Option<String>) {
        if replace.is_some() {
            self.prompt.replace = replace;
        }
        if append.is_some() {
            self.prompt.append = append;
        }
        self.rebuild_system();
    }

    pub async fn shutdown(&self, reason: &str) {
        self.engine.end_session(reason).await;
        if let Some(m) = &self.catalog.mcp {
            m.shutdown().await;
        }
    }
}
