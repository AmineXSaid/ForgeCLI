//! `forge`: argument parsing and dispatch. Handlers stay thin; the work
//! happens in forge-core (session assembly) and forge-engine (the loop).
//! Results go to stdout, diagnostics to stderr; exit statuses are in exit.rs.

mod args;
mod exit;
mod host;
mod mcp_cmd;
mod output;
mod repl;
mod term;

use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::{CommandFactory, Parser};
use forge_core::{build_session, LaunchOptions, Resume};
use forge_engine::{DenyPrompter, EventSink, PermissionPrompter};
use forge_types::sdk::{SdkMessage, SystemMessage};
use forge_types::MessageContent;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use args::{Cli, Command, ConfigAction, InputFormat, Opts, OutputFormat, PermissionPrompts};
use exit::Fail;
use host::{ControlContext, HostPrompter, Input};
use output::{result_message, Out, QuietSink, StreamSink};

fn read_file(p: &PathBuf, what: &str) -> Result<String, Fail> {
    std::fs::read_to_string(p).map_err(|e| Fail::usage(format!("cannot read {what} {}: {e}", p.display())))
}

/// Validate flags and turn them into launch options (no I/O beyond reading named files).
fn launch_options(o: &Opts) -> Result<LaunchOptions, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(format!("cannot read the current directory: {e}")))?;
    let system_prompt = match (&o.system_prompt, &o.system_prompt_file) {
        (Some(_), Some(_)) => return Err(Fail::usage("use either --system-prompt or --system-prompt-file, not both")),
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
        Some(s) => Some(
            serde_json::from_str::<Value>(s)
                .map_err(|e| Fail::usage(format!("--json-schema is not valid JSON: {e}")))?,
        ),
        None => None,
    };
    let resume = match (&o.resume, o.continue_) {
        (Some(id), _) if !id.is_empty() => Resume::Id(id.clone()),
        (Some(_), _) | (None, true) => Resume::Latest,
        (None, false) => Resume::New,
    };
    if o.fork_session && resume == Resume::New {
        return Err(Fail::usage("--fork-session needs --resume or --continue"));
    }
    let setting_sources = match &o.setting_sources {
        Some(s) => Some(forge_config::parse_sources(s).map_err(Fail::usage)?),
        None => None,
    };
    if let Some(m) = &o.permission_mode {
        if forge_permissions::PermissionMode::parse(m).is_none() {
            return Err(Fail::usage(format!(
                "invalid --permission-mode {m:?}: expected one of acceptEdits, auto, bypassPermissions, manual, dontAsk, plan"
            )));
        }
    }
    if let Some(e) = &o.effort {
        if !["low", "medium", "high", "xhigh", "max"].contains(&e.as_str()) {
            return Err(Fail::usage(format!("invalid --effort {e:?}: expected low, medium, high, xhigh or max")));
        }
    }
    if let Some(b) = o.max_budget_usd {
        if !(b > 0.0 && b.is_finite()) {
            return Err(Fail::usage("--max-budget-usd must be a positive number"));
        }
    }
    if o.max_turns == Some(0) {
        return Err(Fail::usage("--max-turns must be at least 1"));
    }
    if let Some(id) = &o.session_id {
        forge_session::validate_session_id(id)
            .map_err(|_| Fail::usage(format!("--session-id {id:?} is not a UUID")))?;
    }
    for d in &o.add_dir {
        if !d.is_dir() {
            return Err(Fail::usage(format!("--add-dir {}: not a directory", d.display())));
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
        autocompact: o.autocompact.clone(),
        agents_json: o.agents.clone(),
        agent: o.agent.clone(),
        sandbox: o.sandbox.clone(),
        mcp_configs: o.mcp_config.clone(),
        strict_mcp_config: o.strict_mcp_config,
        mcp: None,
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
fn print_prompt(arg: Option<String>) -> String {
    let mut piped = String::new();
    if !term::get().stdin_tty {
        std::io::stdin().read_to_string(&mut piped).ok();
    }
    match (arg, piped.trim().is_empty()) {
        (Some(a), true) => a,
        (Some(a), false) => format!("{}\n\n{a}", piped.trim_end()),
        (None, false) => piped,
        (None, true) => String::new(),
    }
}

/// A failure before any run started, in the promised machine format.
fn machine_error(format: OutputFormat, f: &Fail) {
    let doc = json!({
        "type": "error",
        "error": {"message": f.message, "hint": f.hint, "exit_code": f.code},
    });
    match format {
        OutputFormat::Json | OutputFormat::StreamJson => println!("{doc}"),
        OutputFormat::Text => {}
    }
}

async fn run_print(cli_prompt: Option<String>, o: Opts) -> Result<i32, Fail> {
    let stream_out = o.output_format == OutputFormat::StreamJson;
    let stream_in = o.input_format == InputFormat::StreamJson;
    if stream_in && !stream_out {
        return Err(Fail::usage("--input-format=stream-json requires --output-format=stream-json"));
    }
    if o.replay_user_messages && !(stream_in && stream_out) {
        return Err(Fail::usage(
            "--replay-user-messages requires --input-format=stream-json and --output-format=stream-json",
        ));
    }
    if o.include_partial_messages && !stream_out {
        return Err(Fail::usage("--include-partial-messages requires --output-format=stream-json"));
    }
    let checked = launch_options(&o).and_then(|lo| {
        let p = if stream_in { None } else { Some(print_prompt(cli_prompt.clone())) };
        if matches!(&p, Some(p) if p.trim().is_empty()) {
            return Err(Fail::usage("no prompt: pass it as an argument or on stdin")
                .with_hint("forge -p \"explain this repo\"  or  git diff | forge -p \"review this\""));
        }
        Ok((lo, p))
    });
    let (mut lo, first_prompt) = match checked {
        Ok(v) => v,
        Err(f) => {
            machine_error(o.output_format, &f);
            return Err(f);
        }
    };

    let out = Arc::new(Out::default());
    let pending = Arc::new(Mutex::new(Default::default()));
    let host_prompts = stream_in
        && !o.no_input
        && o.permission_prompts == PermissionPrompts::Host
        && o.permission_prompt_tool.as_deref().map(|t| t == "stdio").unwrap_or(true);
    let prompter: Arc<dyn PermissionPrompter> =
        if host_prompts { Arc::new(HostPrompter::new(out.clone(), pending.clone())) } else { Arc::new(DenyPrompter) };

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
        Arc::new(QuietSink { verbose: o.verbose, quiet: o.quiet })
    };

    let (mcp, mcp_warnings) = forge_core::connect_mcp(&lo).await;
    lo.mcp = Some(mcp.clone());
    let session = match build_session(lo, sink, prompter) {
        Ok(s) => s,
        Err(e) => {
            let f = Fail::from(e);
            machine_error(o.output_format, &f);
            mcp.shutdown().await;
            return Err(f);
        }
    };
    if !o.quiet {
        for w in &mcp_warnings {
            eprintln!("{} {w}", term::yellow("forge: warning:"));
        }
    }
    *sink_id.lock().unwrap() = session.session_id.clone();
    if !o.quiet {
        for w in &session.warnings {
            eprintln!("{} {w}", term::yellow("forge: warning:"));
        }
    }
    let session_id = session.session_id.clone();
    let mut engine = session.engine;
    if stream_out {
        out.line(&SdkMessage::System(SystemMessage::init(&session.init)));
    }

    // Ctrl-C interrupts the current turn (the result is still written); a second one exits at once.
    let handle = engine.handle();
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = interrupted.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            handle.interrupt();
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(exit::INTERRUPTED);
            }
        }
    });

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
            mcp: Some(mcp.clone()),
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

    let mut code = exit::OK;
    while let Some(input) = rx.recv().await {
        match input {
            Input::User(content) => {
                let r = engine.submit(content).await;
                let c = exit::for_result(&r);
                if code == exit::OK {
                    code = c;
                }
                match o.output_format {
                    OutputFormat::StreamJson => out.line(&SdkMessage::Result(result_message(&r, &session_id))),
                    OutputFormat::Json => {
                        let mut v = serde_json::to_value(SdkMessage::Result(result_message(&r, &session_id)))
                            .unwrap_or_default();
                        v["exit_code"] = json!(c);
                        println!("{v}");
                    }
                    OutputFormat::Text => {
                        if let Some(b) = &r.prompt_blocked {
                            eprintln!("{} prompt blocked by a UserPromptSubmit hook: {b}", term::red("forge:"));
                        } else if r.is_error {
                            let msg = r
                                .result
                                .clone()
                                .or_else(|| r.errors.first().cloned())
                                .unwrap_or_else(|| "the run failed".into());
                            eprintln!("{} {msg}", term::red("forge:"));
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
    mcp.shutdown().await;
    if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(exit::INTERRUPTED);
    }
    Ok(code)
}

fn run_doctor() -> Result<i32, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(e.to_string()))?;
    let mut problems = 0;
    let mut line = |ok: bool, what: &str, detail: String| {
        if !ok {
            problems += 1;
        }
        let mark = if ok { term::paint("32", "ok  ") } else { term::red("FAIL") };
        println!("{mark} {what:<12} {detail}");
    };
    println!("ForgeCLI {}", forge_core::VERSION);
    let settings = forge_config::load_settings(&forge_config::SettingsOptions::new(&cwd));
    line(
        settings.errors.is_empty(),
        "settings",
        format!(
            "{} layer(s) loaded{}",
            settings.layers.len(),
            if settings.errors.is_empty() {
                String::new()
            } else {
                format!("; invalid: {}", settings.errors.join("; "))
            }
        ),
    );
    let has = |k: &str| std::env::var(k).map(|v| !v.trim().is_empty()).unwrap_or(false);
    let openai = has("FORGE_OPENAI_BASE_URL");
    let creds = has("FORGE_API_KEY") || has("FORGE_AUTH_TOKEN") || settings.str("/apiKeyHelper").is_some();
    line(
        openai || creds,
        "credentials",
        if openai {
            "OpenAI-compatible endpoint (FORGE_OPENAI_BASE_URL)".into()
        } else if creds {
            "found (FORGE_API_KEY, FORGE_AUTH_TOKEN or apiKeyHelper)".into()
        } else {
            "missing: set FORGE_API_KEY".into()
        },
    );
    let endpoint = std::env::var("FORGE_BASE_URL").unwrap_or_else(|_| "default".into());
    line(true, "endpoint", endpoint);
    line(
        which("git"),
        "git",
        if which("git") { "found".into() } else { "not found: git status and worktrees are unavailable".into() },
    );
    let sb = forge_tools::sandbox::backend();
    line(
        true,
        "sandbox",
        match sb {
            Some(b) => format!("{b:?} available (use --sandbox workspace-write)"),
            None => "unavailable: install bubblewrap (Linux) to confine shell commands".into(),
        },
    );
    line(true, "config dir", forge_config::config_dir().display().to_string());
    line(true, "state dir", forge_config::state_dir().display().to_string());
    Ok(if problems == 0 { exit::OK } else { exit::CONFIG })
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}

fn run_config(action: Option<ConfigAction>) -> Result<i32, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(e.to_string()))?;
    let opts = forge_config::SettingsOptions::new(&cwd);
    let settings = forge_config::load_settings(&opts);
    for e in &settings.errors {
        eprintln!("{} {e}", term::yellow("forge: warning: invalid settings file skipped:"));
    }
    let pretty = |v: &Value| serde_json::to_string_pretty(&forge_config::redact(v)).unwrap_or_default();
    match action {
        None | Some(ConfigAction::List { origin: false }) => println!("{}", pretty(&settings.merged)),
        Some(ConfigAction::List { origin: true }) => {
            let mut out = serde_json::Map::new();
            for (k, v) in settings.merged.as_object().into_iter().flatten() {
                let layer = settings.layers.iter().rev().find(|l| l.value.get(k).is_some());
                out.insert(
                    k.clone(),
                    json!({
                        "value": forge_config::redact(&json!({k.as_str(): v}))[k.as_str()].clone(),
                        "source": layer.map(|l| l.source.as_str()),
                        "file": layer.and_then(|l| l.path.as_ref()).map(|p| p.display().to_string()),
                    }),
                );
            }
            println!("{}", serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default());
        }
        Some(ConfigAction::Get { key }) => {
            let ptr = if key.starts_with('/') { key.clone() } else { format!("/{}", key.replace('.', "/")) };
            match settings.get(&ptr) {
                Some(v) => {
                    let last = ptr.rsplit('/').next().unwrap_or("");
                    let shown = forge_config::redact(&json!({last: v}))[last].clone();
                    println!(
                        "{}",
                        if shown.is_string() { shown.as_str().unwrap_or("").to_string() } else { pretty(&shown) }
                    );
                }
                None => {
                    eprintln!("forge: {key} is not set");
                    return Ok(exit::FAILED);
                }
            }
        }
        Some(ConfigAction::Paths) => {
            println!("config  {}", forge_config::config_dir().display());
            println!("state   {}", forge_config::state_dir().display());
            println!("cache   {}", forge_config::cache_dir().display());
            for l in &settings.layers {
                if let Some(p) = &l.path {
                    println!("{:<7} {}", l.source.as_str().trim_end_matches("Settings"), p.display());
                }
            }
        }
    }
    Ok(exit::OK)
}

/// Writing to a closed pipe (`forge ... | head`) ends the process quietly, as in other Unix tools.
fn reset_sigpipe() {
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of SIGPIPE at startup, before any threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn main() {
    reset_sigpipe();
    // clap prints help and version and exits (0) or reports usage errors (2) before anything else starts.
    let cli = Cli::parse();
    term::init(cli.opts.color);
    init_tracing(&cli.opts);
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("forge: cannot start the async runtime: {e}");
            std::process::exit(exit::FAILED);
        }
    };
    let result = rt.block_on(async move {
        match cli.command {
            Some(Command::Doctor) => run_doctor(),
            Some(Command::Config { action }) => run_config(action),
            Some(Command::Completion { shell }) => {
                clap_complete::generate(shell, &mut Cli::command(), "forge", &mut std::io::stdout());
                Ok(exit::OK)
            }
            Some(Command::Mcp { action }) => mcp_cmd::run(action, &cli.opts).await,
            None if cli.opts.print => run_print(cli.prompt, cli.opts).await,
            None => {
                let t = term::get();
                if cli.opts.no_input {
                    Err(Fail::usage("interactive mode needs input, but --no-input is set")
                        .with_hint("Use print mode: forge -p \"<prompt>\""))
                } else if !t.stdin_tty {
                    Err(Fail::usage("interactive mode needs a terminal on stdin").with_hint(
                        "For scripts and pipes use print mode: forge -p \"<prompt>\", or: echo \"<prompt>\" | forge -p",
                    ))
                } else {
                    repl::run(cli.prompt, cli.opts).await
                }
            }
        }
    });
    let code = match result {
        Ok(code) => code,
        Err(f) => {
            eprintln!("{} {}", term::red("forge:"), f.message);
            if let Some(h) = &f.hint {
                eprintln!("  {} {h}", term::dim("hint:"));
            }
            f.code
        }
    };
    rt.shutdown_timeout(std::time::Duration::from_millis(200));
    std::process::exit(code);
}
