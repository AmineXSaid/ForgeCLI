use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InputFormat {
    Text,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PermissionPrompts {
    Host,
    None,
}

/// Forge - an agentic coding CLI. Starts an interactive session by default;
/// use -p/--print for non-interactive output.
#[derive(Debug, Parser)]
#[command(name = "forge", version = concat!(env!("CARGO_PKG_VERSION"), " (ForgeCLI)"), disable_version_flag = true)]
pub struct Cli {
    /// Your prompt
    pub prompt: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub opts: Opts,

    /// Output the version number
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: Option<bool>,
}

#[derive(Debug, Args, Clone)]
pub struct Opts {
    /// Print response and exit (useful for pipes)
    #[arg(short = 'p', long = "print")]
    pub print: bool,
    /// Output format (only with --print)
    #[arg(long = "output-format", value_enum, default_value = "text")]
    pub output_format: OutputFormat,
    /// Input format (only with --print)
    #[arg(long = "input-format", value_enum, default_value = "text")]
    pub input_format: InputFormat,
    /// Include partial message chunks as they arrive (stream-json only)
    #[arg(long = "include-partial-messages")]
    pub include_partial_messages: bool,
    /// Re-emit user messages from stdin on stdout (stream-json in and out)
    #[arg(long = "replay-user-messages")]
    pub replay_user_messages: bool,
    /// Include hook lifecycle events in the output stream
    #[arg(long = "include-hook-events")]
    pub include_hook_events: bool,
    /// Override verbose mode
    #[arg(long)]
    pub verbose: bool,
    /// Enable debug logging, optionally filtered (e.g. "api,hooks")
    #[arg(short = 'd', long = "debug", num_args = 0..=1, default_missing_value = "")]
    pub debug: Option<String>,
    /// Write debug logs to a file (implies --debug)
    #[arg(long = "debug-file")]
    pub debug_file: Option<PathBuf>,

    /// Model for the session: an alias (opus, sonnet, haiku, fable) or a full name
    #[arg(long)]
    pub model: Option<String>,
    /// Fallback model(s), comma-separated, used when the primary is overloaded
    #[arg(long = "fallback-model")]
    pub fallback_model: Option<String>,
    /// Effort level (low, medium, high, xhigh, max)
    #[arg(long)]
    pub effort: Option<String>,
    /// Beta headers to include in API requests
    #[arg(long, num_args = 1..)]
    pub betas: Vec<String>,

    /// Permission mode (acceptEdits, auto, bypassPermissions, manual, dontAsk, plan)
    #[arg(long = "permission-mode")]
    pub permission_mode: Option<String>,
    /// Bypass all permission checks (sandboxes only)
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,
    /// Make bypassing permissions available as an option
    #[arg(long = "allow-dangerously-skip-permissions")]
    pub allow_dangerously_skip_permissions: bool,
    /// Who answers permission prompts in print mode
    #[arg(long = "permission-prompts", value_enum, default_value = "host")]
    pub permission_prompts: PermissionPrompts,
    /// Permission prompt channel (`stdio` = the SDK host)
    #[arg(long = "permission-prompt-tool")]
    pub permission_prompt_tool: Option<String>,
    /// Tools to allow, comma or space separated (e.g. "Bash(git *) Edit")
    #[arg(long = "allowedTools", alias = "allowed-tools", num_args = 1..)]
    pub allowed_tools: Vec<String>,
    /// Tools to deny, comma or space separated
    #[arg(long = "disallowedTools", alias = "disallowed-tools", num_args = 1..)]
    pub disallowed_tools: Vec<String>,
    /// Built-in tools to offer ("" = none, "default" = all)
    #[arg(long, num_args = 0..)]
    pub tools: Option<Vec<String>>,
    /// Additional directories to allow tool access to
    #[arg(long = "add-dir", num_args = 1..)]
    pub add_dir: Vec<PathBuf>,

    /// System prompt for the session
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,
    /// Read the system prompt from a file
    #[arg(long = "system-prompt-file")]
    pub system_prompt_file: Option<PathBuf>,
    /// Append to the default system prompt
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Option<String>,
    /// Append the contents of a file to the system prompt
    #[arg(long = "append-system-prompt-file")]
    pub append_system_prompt_file: Option<PathBuf>,
    /// Move per-machine sections from the system prompt into the first user message
    #[arg(long = "exclude-dynamic-system-prompt-sections")]
    pub exclude_dynamic_system_prompt_sections: bool,

    /// Continue the most recent conversation in this directory
    #[arg(short = 'c', long = "continue")]
    pub continue_: bool,
    /// Resume a conversation by session ID
    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,
    /// When resuming, create a new session ID
    #[arg(long = "fork-session")]
    pub fork_session: bool,
    /// Use a specific session ID (a UUID)
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    /// Do not save the session to disk (print mode)
    #[arg(long = "no-session-persistence")]
    pub no_session_persistence: bool,
    /// Display name for the session
    #[arg(short = 'n', long = "name")]
    pub name: Option<String>,

    /// Maximum number of agentic turns (print mode)
    #[arg(long = "max-turns")]
    pub max_turns: Option<u32>,
    /// Maximum dollar amount to spend on API calls (print mode)
    #[arg(long = "max-budget-usd")]
    pub max_budget_usd: Option<f64>,
    /// JSON Schema for structured output
    #[arg(long = "json-schema")]
    pub json_schema: Option<String>,

    /// Settings file path or JSON string
    #[arg(long)]
    pub settings: Option<String>,
    /// Setting sources to load (user, project, local)
    #[arg(long = "setting-sources")]
    pub setting_sources: Option<String>,
    /// Minimal mode: no hooks or memory files
    #[arg(long)]
    pub bare: bool,
    /// Start with all customizations disabled
    #[arg(long = "safe-mode")]
    pub safe_mode: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check the health of the installation
    Doctor,
    /// Show the effective settings
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the merged settings
    List,
    /// Print one setting (JSON pointer, e.g. /permissions/defaultMode)
    Get { key: String },
}
