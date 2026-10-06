mod args;
mod host;
mod output;
mod repl;

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use clap::Parser;
use forge_core::{build_session, LaunchOptions, Resume};
use forge_engine::{DenyPrompter, EventSink, PermissionPrompter};
use forge_types::sdk::{SdkMessage, SystemMessage};
use forge_types::MessageContent;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use args::{Cli, Command, InputFormat, Opts, OutputFormat, PermissionPrompts};
use host::{ControlContext, HostPrompter, Input};
use output::{result_message, Out, QuietSink, StreamSink};

fn read_file(p: &PathBuf, what: &str) -> anyhow::Result<String> {
    std::fs::read_to_string(p).with_context(|| format!("cannot read {what} {}", p.display()))
}

fn launch_options(o: &Opts) -> anyhow::Result<LaunchOptions> {
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let system_prompt = match (&o.system_prompt, &o.system_prompt_file) {
        (Some(_), Some(_)) => bail!("use either --system-prompt or --system-prompt-file, not both"),
        (Some(s), None) => Some(s.clone()),
        (None, Some(f)) => Some(read_file(f, "system prompt file")?),
        (None, None) => None,
    };
    let append = match (&o.append_system_prompt, &o.append_system_prompt_file) {
        (Some(a), Some(f)) => Some(format!("{a}\n\n{}", read_file(f, "append system prompt file")?)),
        (Some(a), None) => Some(a.clone()),
        (None, Some(f)) => Some(read_file(f, "append system prompt file")?),
        (None, None) => None,
    };
    let json_schema = match &o.json_schema {
        Some(s) => Some(serde_json::from_str::<Value>(s).context("--json-schema is not valid JSON")?),
        None => None,
    };
    let resume = match (&o.resume, o.continue_) {
        (Some(id), _) if !id.is_empty() => Resume::Id(id.clone()),
        (Some(_), _) | (None, true) => Resume::Latest,
        (None, false) => Resume::New,
    };
    if o.fork_session && resume == Resume::New {
        bail!("--fork-session needs --resume or --continue");
    }
    let setting_sources = match &o.setting_sources {
        Some(s) => Some(forge_config::parse_sources(s).map_err(anyhow::Error::msg)?),
        None => None,
    };
    if let Some(m) = &o.permission_mode {
        if forge_permissions::PermissionMode::parse(m).is_none() {
            bail!("invalid --permission-mode {m:?} (acceptEdits, auto, bypassPermissions, manual, dontAsk, plan)");
        }
    }
    if let Some(e) = &o.effort {
        if !["low", "medium", "high", "xhigh", "max"].contains(&e.as_str()) {
            bail!("invalid --effort {e:?} (low, medium, high, xhigh, max)");
        }
    }
    Ok(LaunchOptions {
        cwd,
        model: o.model.clone(),
        fallback_models: o.fallback_model.iter().cloned().collect(),
        effort: o.effort.clone(),
        permission_mode: o.permission_mode.clone(),
        dangerously_skip_permissions: o.dangerously_skip_permissions,
        allow_dangerously_skip_permissions: o.allow_dangerously_skip_permissions,
        allowed_tools: o.allowed_tools.clone(),
        disallowed_tools: o.disallowed_tools.clone(),
        tools: o.tools.clone(),
        add_dirs: o.add_dir.clone(),
        system_prompt,
        append_system_prompt: append,
        exclude_dynamic_system_prompt_sections: o.exclude_dynamic_system_prompt_sections,
        settings: o.settings.clone(),
        setting_sources,
        max_turns: o.max_turns,
        max_budget_usd: o.max_budget_usd,
        json_schema,
        resume,
        fork_session: o.fork_session,
        session_id: o.session_id.clone(),
        no_session_persistence: o.no_session_persistence,
        bare: o.bare || o.safe_mode,
        betas: o.betas.clone(),
        provider: None,
        store_root: None,
    })
}

fn init_tracing(o: &Opts) {
    if o.debug.is_none() && o.debug_file.is_none() && std::env::var_os("FORGE_LOG").is_none() {
        return;
    }
    let filter = std::env::var("FORGE_LOG").unwrap_or_else(|_| "debug".into());
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    match &o.debug_file {
        Some(path) => {
            if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                builder.with_writer(Mutex::new(f)).init();
            }
        }
        None => builder.with_writer(std::io::stderr).init(),
    }
}

/// The prompt for print mode: the argument and/or piped stdin.
fn print_prompt(arg: Option<String>, input: InputFormat) -> anyhow::Result<String> {
    let mut piped = String::new();
    if input == InputFormat::Text && !std::io::stdin().is_terminal() {
        std::io::stdin().read_to_string(&mut piped).ok();
    }
    let prompt = match (arg, piped.trim().is_empty()) {
        (Some(a), true) => a,
        (Some(a), false) => format!("{}\n\n{a}", piped.trim_end()),
        (None, false) => piped,
        (None, true) => String::new(),
    };
    Ok(prompt)
}

async fn run_print(cli_prompt: Option<String>, o: Opts) -> anyhow::Result<i32> {
    let stream_out = o.output_format == OutputFormat::StreamJson;
    let stream_in = o.input_format == InputFormat::StreamJson;
    if stream_in && !stream_out {
        bail!("--input-format=stream-json requires --output-format=stream-json");
    }
    if o.replay_user_messages && !(stream_in && stream_out) {
        bail!("--replay-user-messages requires --input-format=stream-json and --output-format=stream-json");
    }
    if o.include_partial_messages && !stream_out {
        bail!("--include-partial-messages requires --output-format=stream-json");
    }
    let mut lo = launch_options(&o)?;
    let first_prompt = if stream_in { None } else { Some(print_prompt(cli_prompt, o.input_format)?) };
    if matches!(&first_prompt, Some(p) if p.trim().is_empty()) {
        bail!("Input must be provided either through stdin or as a prompt argument when using --print");
    }

    let out = Arc::new(Out::default());
    let pending = Arc::new(Mutex::new(Default::default()));
    let host_prompts = stream_in
        && o.permission_prompts == PermissionPrompts::Host
        && o.permission_prompt_tool.as_deref().map(|t| t == "stdio").unwrap_or(true);
    let prompter: Arc<dyn PermissionPrompter> =
        if host_prompts { Arc::new(HostPrompter::new(out.clone(), pending.clone())) } else { Arc::new(DenyPrompter) };

    // The session id is needed by the sink; resolve it first for new sessions.
    if lo.session_id.is_none() && lo.resume == Resume::New {
        lo.session_id = Some(uuid::Uuid::new_v4().to_string());
    }
    let sink_id = Arc::new(Mutex::new(lo.session_id.clone().unwrap_or_default()));
    let sink: Arc<dyn EventSink> = if stream_out {
        Arc::new(StreamSink {
            out: out.clone(),
            session_id: sink_id.clone(),
            include_partial: o.include_partial_messages,
            replay_user_messages: o.replay_user_messages,
            verbose: o.verbose,
        })
    } else {
        Arc::new(QuietSink { verbose: o.verbose })
    };

    let session = match build_session(lo, sink, prompter) {
        Ok(s) => s,
        Err(e) => {
            if stream_out {
                let r = json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "errors": [e.to_string()]});
                println!("{r}");
            }
            return Err(e.into());
        }
    };
    *sink_id.lock().unwrap() = session.session_id.clone();
    for w in &session.warnings {
        eprintln!("forge: {w}");
    }
    let session_id = session.session_id.clone();
    let mut engine = session.engine;

    if stream_out {
        out.line(&SdkMessage::System(SystemMessage::init(&session.init)));
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();
    if stream_in {
        let models: Vec<Value> = forge_api::models::MODELS
            .iter()
            .map(|m| {
                json!({"value": m.id, "displayName": m.display_name, "supportsEffort": m.supports_effort(),
                            "supportedEffortLevels": m.effort_levels})
            })
            .collect();
        let ctx = Arc::new(ControlContext {
            out: out.clone(),
            pending: pending.clone(),
            handle: engine.handle(),
            history: engine.history().clone(),
            init_response: json!({
                "commands": [],
                "output_style": "default",
                "available_output_styles": ["default"],
                "models": models,
                "pid": std::process::id(),
            }),
        });
        tokio::spawn(host::read_stdin(ctx, tx));
    } else {
        let _ = tx.send(Input::User(MessageContent::Text(first_prompt.unwrap_or_default())));
        let _ = tx.send(Input::Eof);
    }

    let mut exit = 0;
    while let Some(input) = rx.recv().await {
        match input {
            Input::User(content) => {
                let r = engine.submit(content).await;
                if r.is_error {
                    exit = 1;
                }
                match o.output_format {
                    OutputFormat::StreamJson => out.line(&SdkMessage::Result(result_message(&r, &session_id))),
                    OutputFormat::Json => println!(
                        "{}",
                        serde_json::to_string(&json!({
                            "type": "result",
                            "subtype": r.subtype,
                            "is_error": r.is_error,
                            "duration_ms": r.duration_ms,
                            "duration_api_ms": r.duration_api_ms,
                            "num_turns": r.num_turns,
                            "result": r.result.clone().or(r.prompt_blocked.clone()),
                            "stop_reason": r.stop_reason,
                            "session_id": session_id,
                            "total_cost_usd": r.total_cost_usd,
                            "usage": r.usage,
                            "modelUsage": r.model_usage,
                            "permission_denials": r.permission_denials,
                            "structured_output": r.structured_output,
                            "errors": r.errors,
                        }))?
                    ),
                    OutputFormat::Text => {
                        if let Some(b) = &r.prompt_blocked {
                            eprintln!("{b}");
                        } else if let Some(t) = &r.result {
                            println!("{t}");
                        }
                    }
                }
            }
            Input::SystemPrompt { replace, append } => {
                let env = forge_engine::EnvInfo::collect(&engine.tool_ctx().project_dir, &[], &engine.handle().model());
                let opts = forge_engine::SystemPromptOptions { replace, append, ..Default::default() };
                engine.set_system(forge_engine::build_system(&opts, &env).0);
            }
            Input::Eof => break,
        }
    }
    engine.end_session("other").await;
    Ok(exit)
}

async fn run_doctor() -> anyhow::Result<i32> {
    let cwd = std::env::current_dir()?;
    println!("ForgeCLI {}", forge_core::VERSION);
    println!("  home:        {}", forge_config::forge_home().display());
    println!("  project:     {}", cwd.display());
    let settings = forge_config::load_settings(&forge_config::SettingsOptions::new(&cwd));
    println!("  settings:    {} layer(s) loaded", settings.layers.len());
    for e in &settings.errors {
        println!("    ! {e}");
    }
    let key = std::env::var("FORGE_API_KEY").is_ok() || std::env::var("FORGE_AUTH_TOKEN").is_ok();
    let openai = std::env::var("FORGE_OPENAI_BASE_URL").is_ok();
    println!(
        "  credentials: {}",
        if openai {
            "OpenAI-compatible endpoint"
        } else if key {
            "set"
        } else {
            "missing (set FORGE_API_KEY)"
        }
    );
    println!("  git:         {}", if forge_git_present() { "found" } else { "not found" });
    Ok(if key || openai { 0 } else { 1 })
}

fn forge_git_present() -> bool {
    std::process::Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn run_config(action: Option<args::ConfigAction>) -> anyhow::Result<i32> {
    let cwd = std::env::current_dir()?;
    let settings = forge_config::load_settings(&forge_config::SettingsOptions::new(&cwd));
    match action {
        None | Some(args::ConfigAction::List) => println!("{}", serde_json::to_string_pretty(&settings.merged)?),
        Some(args::ConfigAction::Get { key }) => {
            let ptr = if key.starts_with('/') { key } else { format!("/{}", key.replace('.', "/")) };
            match settings.get(&ptr) {
                Some(v) => println!("{}", serde_json::to_string_pretty(v)?),
                None => return Ok(1),
            }
        }
    }
    Ok(0)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing(&cli.opts);
    let result = match cli.command {
        Some(Command::Doctor) => run_doctor().await,
        Some(Command::Config { action }) => run_config(action),
        None if cli.opts.print => run_print(cli.prompt, cli.opts).await,
        None => repl::run(cli.prompt, cli.opts).await,
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}
