//! Builds a ready-to-run session from launch options and settings.

pub mod commands;
pub mod debug;
pub mod doctor;
pub mod driver;
pub mod goal;
pub mod prompt;
pub mod web;

pub use driver::{Driver, Flow, Report};
pub use prompt::PromptSpec;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forge_api::{MessagesConfig, MessagesProvider, OpenAiConfig, OpenAiProvider, Provider};
use forge_config::{load_memory, load_settings, LoadedSettings, MemoryKind, SettingSource, SettingsOptions};
use forge_engine::{Engine, EngineConfig, EngineParts, EnvInfo, EventSink, PermissionPrompter, Pricing};
use forge_hooks::{HookBase, HookRunner, HooksConfig};
use forge_permissions::{PermissionMode, Rule, RuleSet};
use forge_session::{FileHistory, LoadedSession, SessionStore, Transcript};
use forge_tools::{ToolContext, ToolRegistry};
use forge_types::sdk::{InitInfo, McpServerStatus};
use serde_json::{json, Value};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{0}")]
    Config(String),
    /// No usable credentials or endpoint.
    #[error("{0}")]
    Auth(String),
    #[error(transparent)]
    Session(#[from] forge_session::SessionError),
    #[error(transparent)]
    Engine(#[from] forge_engine::EngineError),
    #[error(transparent)]
    Api(#[from] forge_api::ApiError),
}

/// How to resume (`--continue`, `--resume [id]`).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Resume {
    #[default]
    New,
    Latest,
    Id(String),
}

/// Everything the command line can say about a session.
#[derive(Clone, Default)]
pub struct LaunchOptions {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub fallback_models: Vec<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub dangerously_skip_permissions: bool,
    pub allow_dangerously_skip_permissions: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// `--tools`: `None` = all built-ins, `Some([])` = none.
    pub tools: Option<Vec<String>>,
    pub add_dirs: Vec<PathBuf>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub exclude_dynamic_system_prompt_sections: bool,
    pub settings: Option<String>,
    pub setting_sources: Option<Vec<SettingSource>>,
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    pub json_schema: Option<Value>,
    pub resume: Resume,
    pub fork_session: bool,
    pub session_id: Option<String>,
    pub no_session_persistence: bool,
    /// `--bare` / `--safe-mode`: no hooks, no memory files.
    pub bare: bool,
    pub betas: Vec<String>,
    /// `--autocompact <auto|tokens>`: the window compaction thresholds use.
    pub autocompact: Option<String>,
    /// `--agents <json>`: extra agent definitions.
    pub agents_json: Option<String>,
    /// `--agent <name>`: run the main session as this agent.
    pub agent: Option<String>,
    /// `--sandbox <off|read-only|workspace-write>` for shell commands.
    pub sandbox: Option<String>,
    /// `--plugin-dir`: plugin directories (with the `pluginDirs` setting).
    pub plugin_dirs: Vec<PathBuf>,
    /// `--worktree [name]`: run in a new git worktree (`Some("")` = a generated name).
    pub worktree: Option<String>,
    /// `--mcp-config`: server config files or JSON strings.
    pub mcp_configs: Vec<String>,
    /// `--strict-mcp-config`: only the servers from `mcp_configs`.
    pub strict_mcp_config: bool,
    /// Connected MCP servers (from [`connect_mcp`]); their tools join the session.
    pub mcp: Option<Arc<forge_mcp::McpManager>>,
    /// Replace the provider (tests, embedding).
    pub provider: Option<Arc<dyn Provider>>,
    /// Where sessions live (default `~/.forge/projects`).
    pub store_root: Option<PathBuf>,
}

/// WebFetch (always) and WebSearch (when the provider can search), backed by a small model.
fn web_tools(provider: &Arc<dyn Provider>, settings: &LoadedSettings) -> Vec<Arc<dyn forge_tools::Tool>> {
    let backend = Arc::new(web::ProviderWeb {
        provider: provider.clone(),
        model: settings
            .str("/smallFastModel")
            .map(forge_api::resolve_model)
            .unwrap_or_else(|| forge_api::models::SMALL_FAST_MODEL.to_string()),
        search: provider.name() != "openai",
        search_tool: settings.str("/webSearch/toolType").unwrap_or("web_search_20250305").to_string(),
    });
    let mut out: Vec<Arc<dyn forge_tools::Tool>> =
        vec![Arc::new(forge_tools::builtin::WebFetch::new(Some(backend.clone())))];
    if forge_tools::builtin::WebBackend::can_search(backend.as_ref()) {
        out.push(Arc::new(forge_tools::builtin::WebSearch { backend }));
    }
    out
}

struct Worktree {
    name: String,
    path: PathBuf,
    main_repo: PathBuf,
}

/// `--worktree [name]` (contract C10): `<repo>/.forge/worktrees/<name>` on branch `forge/<name>`,
/// created from HEAD, or reused when it already exists.
fn enter_worktree(cwd: &Path, name: &str, session_id: Option<&str>) -> Result<Worktree, CoreError> {
    let root = forge_git::repo_root(cwd)
        .ok_or_else(|| CoreError::Config("--worktree needs a git repository; run forge inside one".into()))?;
    let name = match name.trim() {
        "" => {
            let id = session_id.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            format!("session-{}", &id[..8.min(id.len())])
        }
        n => n.to_string(),
    };
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') || name.starts_with('.') {
        return Err(CoreError::Config(format!("--worktree name {name:?}: use letters, digits, '-', '_' and '.'")));
    }
    let path = root.join(".forge").join("worktrees").join(&name);
    if !path.exists() {
        forge_git::add_worktree(&root, &path, &format!("forge/{name}"))
            .map_err(|e| CoreError::Config(format!("--worktree: git could not create it: {e}")))?;
        let _ = forge_git::exclude(&root, ".forge/worktrees/");
    }
    Ok(Worktree { name, path: path.canonicalize().unwrap_or(path), main_repo: root })
}

/// Load the session's settings the way [`build_session`] does.
fn session_settings(opts: &LaunchOptions, cwd: &Path) -> LoadedSettings {
    let mut sopts = SettingsOptions::new(cwd);
    if let Some(s) = &opts.setting_sources {
        sopts.sources = s.clone();
    }
    sopts.flag = opts.settings.clone();
    load_settings(&sopts)
}

/// Which MCP servers this session would start (contract C16). `--bare` keeps only `--mcp-config`.
pub fn resolve_mcp(opts: &LaunchOptions) -> forge_mcp::Resolved {
    let cwd = opts.cwd.canonicalize().unwrap_or(opts.cwd.clone());
    let settings = session_settings(opts, &cwd);
    // Plugin servers count as explicitly chosen, like --mcp-config.
    let mut configs: Vec<String> = session_plugins(opts, &settings, &cwd, &mut vec![])
        .iter()
        .filter_map(|p| p.mcp_file())
        .map(|f| f.display().to_string())
        .collect();
    configs.extend(opts.mcp_configs.iter().cloned());
    forge_mcp::resolve(&settings, &cwd, &configs, opts.strict_mcp_config || opts.bare)
}

/// `--plugin-dir` plus the `pluginDirs` setting (relative to the project). None in `--bare`.
fn session_plugins(
    opts: &LaunchOptions,
    settings: &LoadedSettings,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Vec<forge_agents::Plugin> {
    if opts.bare {
        return vec![];
    }
    let mut dirs = opts.plugin_dirs.clone();
    dirs.extend(settings.strings("/pluginDirs").into_iter().map(|d| forge_permissions::normalize(Path::new(&d), cwd)));
    forge_agents::load_plugins(&dirs, warnings)
}

/// Connect the session's MCP servers. Failures are reported in the manager, never fatal.
pub async fn connect_mcp(opts: &LaunchOptions) -> (Arc<forge_mcp::McpManager>, Vec<String>) {
    let resolved = resolve_mcp(opts);
    let cwd = opts.cwd.canonicalize().unwrap_or(opts.cwd.clone());
    let manager = forge_mcp::McpManager::connect(&resolved, &forge_mcp::ConnectOptions::new(&cwd)).await;
    let mut warnings = resolved.warnings;
    warnings.extend(manager.warnings());
    (Arc::new(manager), warnings)
}

/// A built session, ready for `engine.submit`.
pub struct Session {
    pub engine: Engine,
    pub init: InitInfo,
    /// What the system prompt is built from, for rebuilding it later.
    pub prompt: PromptSpec,
    /// Custom slash commands, skills, output styles and plugins (M5).
    pub commands: Vec<forge_agents::CommandDef>,
    pub skills: Vec<forge_agents::SkillDef>,
    pub styles: Vec<forge_agents::OutputStyle>,
    pub plugins: Vec<forge_agents::Plugin>,
    pub agents: Vec<forge_agents::AgentDef>,
    pub settings: LoadedSettings,
    pub warnings: Vec<String>,
    pub session_id: String,
    pub resumed: Option<LoadedSession>,
}

/// The verification loop's settings: `verification.{enabled, commands,
/// maxReminders}`, with commands detected from the project's manifests when
/// none are set. `FORGE_VERIFY=0` turns it off (for A/B runs).
pub(crate) fn verify_config(settings: &LoadedSettings, cwd: &Path) -> Option<forge_engine::VerifyConfig> {
    let off = env_nonempty("FORGE_VERIFY").map(|v| matches!(v.as_str(), "0" | "false" | "off" | "no")).unwrap_or(false);
    if off || settings.bool("/verification/enabled") == Some(false) {
        return None;
    }
    let configured = settings.strings("/verification/commands");
    Some(forge_engine::VerifyConfig {
        commands: if configured.is_empty() { forge_engine::detect_checks(cwd) } else { configured },
        max_reminders: settings
            .get("/verification/maxReminders")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(1),
    })
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

/// The provider for this session: an OpenAI-compatible endpoint when one is
/// configured, the Messages API otherwise.
pub fn make_provider(settings: &LoadedSettings, betas: &[String]) -> Result<Arc<dyn Provider>, CoreError> {
    let openai_url =
        env_nonempty("FORGE_OPENAI_BASE_URL").or_else(|| settings.str("/openai/baseUrl").map(str::to_string));
    if let Some(url) = openai_url {
        let cfg = OpenAiConfig { base_url: url, api_key: env_nonempty("FORGE_OPENAI_API_KEY"), ..Default::default() };
        return Ok(Arc::new(OpenAiProvider::new(cfg)?));
    }
    let mut cfg = MessagesConfig::from_env();
    if cfg.base_url == forge_api::messages::DEFAULT_BASE_URL {
        if let Some(u) = settings.str("/baseUrl") {
            cfg.base_url = u.to_string();
        }
    }
    if !cfg.has_credentials() {
        if let Some(helper) = settings.str("/apiKeyHelper") {
            cfg.api_key = run_key_helper(helper);
        }
    }
    if !cfg.has_credentials() {
        return Err(CoreError::Auth(
            "no API credentials found. Set FORGE_API_KEY (or FORGE_AUTH_TOKEN for a gateway), or \
             FORGE_OPENAI_BASE_URL for an OpenAI-compatible endpoint; `forge doctor` shows the current setup"
                .into(),
        ));
    }
    cfg.betas = betas.to_vec();
    Ok(Arc::new(MessagesProvider::new(cfg)?))
}

/// `apiKeyHelper`: a command whose stdout is the key.
fn run_key_helper(cmd: &str) -> Option<String> {
    let out = std::process::Command::new("/bin/sh").arg("-c").arg(cmd).output().ok()?;
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !key.is_empty()).then_some(key)
}

/// Tools named in `--disallowedTools` without a specifier are removed outright.
fn removed_tools(disallowed: &[String]) -> Vec<String> {
    disallowed.iter().flat_map(|s| forge_permissions::split_rule_list(s)).filter(|r| !r.contains('(')).collect()
}

fn pricing_from_settings(s: &LoadedSettings) -> HashMap<String, Pricing> {
    let mut out = HashMap::new();
    if let Some(m) = s.get("/modelPricing").and_then(Value::as_object) {
        for (model, p) in m {
            let f = |k: &str| p.get(k).and_then(Value::as_f64);
            if let (Some(input), Some(output)) = (f("input"), f("output")) {
                out.insert(
                    model.clone(),
                    Pricing {
                        input,
                        output,
                        cache_read: f("cacheRead").unwrap_or(input * 0.1),
                        cache_write: f("cacheWrite").unwrap_or(input * 1.25),
                    },
                );
            }
        }
    }
    out
}

fn memory_context(cwd: &Path) -> Option<String> {
    let files = load_memory(cwd);
    if files.is_empty() {
        return None;
    }
    let mut s = String::from(
        "As you answer, follow the instructions below. They override default behaviour; follow them exactly.\n",
    );
    for f in files {
        let what = match f.kind {
            MemoryKind::User => "the user's private global instructions for all projects",
            MemoryKind::Project => "project instructions, checked into the codebase",
            MemoryKind::Local => "the user's private project instructions, not checked in",
        };
        s.push_str(&format!("\nContents of {} ({what}):\n\n{}\n", f.path.display(), f.content.trim_end()));
    }
    Some(s)
}

/// Build a session. `sink` receives engine events; `prompter` answers permission prompts.
pub fn build_session(
    opts: LaunchOptions,
    sink: Arc<dyn EventSink>,
    prompter: Arc<dyn PermissionPrompter>,
) -> Result<Session, CoreError> {
    let mut cwd = opts.cwd.canonicalize().unwrap_or(opts.cwd.clone());
    let mut warnings = vec![];
    let worktree = match &opts.worktree {
        Some(name) => {
            let w = enter_worktree(&cwd, name, opts.session_id.as_deref())?;
            cwd = w.path.clone();
            Some(w)
        }
        None => None,
    };

    // Settings.
    let settings = session_settings(&opts, &cwd);
    warnings.extend(settings.errors.iter().map(|e| format!("settings: {e}")));

    let provider = match opts.provider.clone() {
        Some(p) => p,
        None => make_provider(&settings, &opts.betas)?,
    };

    // Plugins, commands, skills and output styles (M5).
    let plugins = session_plugins(&opts, &settings, &cwd, &mut warnings);
    let commands = if opts.bare { vec![] } else { forge_agents::load_commands(&cwd, &plugins, &mut warnings) };
    let skills = if opts.bare { vec![] } else { forge_agents::load_skills(&cwd, &plugins, &mut warnings) };
    let styles = forge_agents::load_styles(&cwd, &plugins, &mut warnings);
    let style_name = settings.str("/outputStyle").unwrap_or("default").to_string();
    let style = match forge_agents::styles::find(&styles, &style_name) {
        Some(s) => s.clone(),
        None => {
            warnings.push(format!("outputStyle {style_name:?} not found; using default"));
            styles[0].clone()
        }
    };
    let flag_agents = match &opts.agents_json {
        Some(raw) => {
            let raw = if raw.trim_start().starts_with('{') {
                raw.clone()
            } else {
                std::fs::read_to_string(raw)
                    .map_err(|e| CoreError::Config(format!("--agents: cannot read {raw}: {e}")))?
            };
            forge_agents::parse_agents_json(&raw).map_err(CoreError::Config)?
        }
        None => vec![],
    };
    let agents = if opts.bare {
        forge_agents::builtin_agents()
    } else {
        forge_agents::load_agents(&cwd, &plugins, &flag_agents, &mut warnings)
    };
    let main_agent = match &opts.agent {
        Some(name) => Some(
            agents
                .iter()
                .find(|a| &a.name == name)
                .cloned()
                .ok_or_else(|| CoreError::Config(format!("--agent: no agent named {name:?}")))?,
        ),
        None => None,
    };

    // Session identity and history.
    let store =
        SessionStore::new(opts.store_root.clone().unwrap_or_else(|| forge_session::forge_home().join("projects")));
    let mut resumed: Option<LoadedSession> = match &opts.resume {
        Resume::New => None,
        Resume::Latest => match store.latest(&cwd) {
            Some(s) => Some(LoadedSession::load(&s.path, None)?),
            None => {
                warnings.push("No conversation found to continue; starting a new one.".into());
                None
            }
        },
        Resume::Id(id) => Some(LoadedSession::load(&store.find(id)?, None)?),
    };
    let session_id = match (&resumed, opts.fork_session, &opts.session_id) {
        (Some(_), true, Some(id)) | (None, _, Some(id)) => id.clone(),
        (Some(r), false, _) => r.session_id.clone(),
        _ => uuid::Uuid::new_v4().to_string(),
    };
    forge_session::validate_session_id(&session_id)?;
    let git_branch = forge_git::current_branch(&cwd);
    let transcript = Arc::new(Transcript::create(&store, &cwd, &session_id, git_branch, !opts.no_session_persistence)?);
    if let Some(r) = &resumed {
        if r.session_id != session_id {
            transcript.write_fork_of(r);
        }
    }
    if let Some(w) = &worktree {
        transcript.append_meta(json!({"worktree": {"name": w.name, "path": w.path, "mainRepo": w.main_repo}}));
        if !opts.no_session_persistence {
            store.register_project(&cwd, Some(&w.main_repo))?;
        }
    }

    // Working directories: flags, settings, and whatever the resumed session had.
    let mut add_dirs: Vec<PathBuf> = opts.add_dirs.iter().map(|d| forge_permissions::normalize(d, &cwd)).collect();
    for d in settings.strings("/permissions/additionalDirectories") {
        add_dirs.push(forge_permissions::normalize(Path::new(&d), &cwd));
    }
    if let Some(r) = &resumed {
        add_dirs.extend(r.additional_dirs.iter().cloned());
    }
    add_dirs.sort();
    add_dirs.dedup();
    if !add_dirs.is_empty() {
        transcript.append_meta(json!({"additionalDirectories": add_dirs}));
    }

    // Permissions.
    let mode_name = if opts.dangerously_skip_permissions {
        Some("bypassPermissions".to_string())
    } else {
        opts.permission_mode.clone().or_else(|| settings.str("/permissions/defaultMode").map(str::to_string))
    };
    let mut mode = match mode_name.as_deref() {
        Some(m) => {
            PermissionMode::parse(m).ok_or_else(|| CoreError::Config(format!("unknown permission mode {m:?}")))?
        }
        None => PermissionMode::Default,
    };
    let bypass_disabled =
        settings.managed().and_then(|m| m.pointer("/permissions/disableBypassPermissionsMode")).and_then(Value::as_str)
            == Some("disable");
    if mode == PermissionMode::BypassPermissions && bypass_disabled {
        warnings.push("bypassPermissions is disabled by managed settings; using default mode.".into());
        mode = PermissionMode::Default;
    }
    let mut allow = settings.strings("/permissions/allow");
    allow.extend(opts.allowed_tools.iter().cloned());
    let ask = settings.strings("/permissions/ask");
    let mut deny = settings.strings("/permissions/deny");
    deny.extend(
        opts.disallowed_tools.iter().flat_map(|s| forge_permissions::split_rule_list(s)).filter(|r| r.contains('(')),
    );
    let (rules, rule_errors): (RuleSet, _) = RuleSet::from_strings(&allow, &ask, &deny);
    warnings.extend(rule_errors.iter().map(|e| e.to_string()));
    let mut permissions = forge_permissions::Engine::new(mode, rules, &cwd, &add_dirs);
    // Output too long for a tool result is saved here, and may be read back without a prompt.
    let spill_dir = forge_config::cache_dir().join("tool-output").join(&session_id);
    permissions.add_read_dir(&spill_dir);

    // Tools.
    let mut tools = ToolRegistry::new();
    forge_tools::builtin::register_core(&mut tools);
    let web_tools = web_tools(&provider, &settings);
    for t in &web_tools {
        tools.register(t.clone());
    }
    if skills.iter().any(|s| s.model_invocable) {
        tools.register(Arc::new(forge_agents::SkillTool { skills: skills.clone() }));
    }
    let agent_store =
        (!opts.no_session_persistence).then(|| SessionStore::new(store.root.join("agents").join(&session_id)));
    let agent_rt_slot: Arc<std::sync::OnceLock<Arc<forge_agents::AgentRuntime>>> = Arc::new(std::sync::OnceLock::new());
    if let Some(list) = &opts.tools {
        let names: Vec<String> = list.iter().flat_map(|s| forge_permissions::split_rule_list(s)).collect();
        if !(names.len() == 1 && names[0] == "default") {
            tools.retain(|n| names.iter().any(|x| x == n));
        }
    }
    // MCP tools join after `--tools` (which names built-ins) and before `--disallowedTools`.
    if let Some(m) = &opts.mcp {
        for t in m.tools() {
            tools.register(t);
        }
    }
    let removed = removed_tools(&opts.disallowed_tools);
    tools.retain(|n| !removed.iter().any(|r| Rule::parse(r).map(|rule| rule.covers_tool(n)).unwrap_or(false)));

    let mut tool_ctx = ToolContext::new(&cwd);
    tool_ctx.session_id = session_id.clone();
    tool_ctx.spill_dir = Some(spill_dir);
    {
        let mut wd = tool_ctx.working_dirs.write().unwrap();
        wd.extend(add_dirs.iter().cloned());
    }
    // OS sandbox for shell commands: flag > FORGE_SANDBOX > settings.
    let sandbox_mode = opts
        .sandbox
        .clone()
        .or_else(|| env_nonempty("FORGE_SANDBOX"))
        .or_else(|| settings.str("/sandbox/mode").map(str::to_string));
    if let Some(m) = sandbox_mode {
        let parsed = forge_tools::sandbox::SandboxMode::parse(&m).ok_or_else(|| {
            CoreError::Config(format!("unknown sandbox mode {m:?}: expected off, read-only or workspace-write"))
        })?;
        if let Some(mode) = parsed {
            if forge_tools::sandbox::backend().is_none() {
                warnings.push(format!(
                    "sandbox {} requested but unavailable ({}); shell commands will ask for approval instead",
                    mode.as_str(),
                    if cfg!(target_os = "linux") { "install bubblewrap (bwrap)" } else { "no sandbox-exec" }
                ));
            } else {
                tool_ctx.set_sandbox(Some(forge_tools::sandbox::SandboxPolicy {
                    mode,
                    network: settings.bool("/sandbox/network").unwrap_or(false),
                    writable_roots: vec![],
                    extra_writable: settings.strings("/sandbox/writableRoots").into_iter().map(PathBuf::from).collect(),
                }));
            }
        }
    }
    let mut env: HashMap<String, String> = settings.env().into_iter().collect();
    env.insert("FORGE_SESSION_ID".into(), session_id.clone());
    tool_ctx.env = Arc::new(env);

    // Hooks.
    let mut hooks_value = settings.get("/hooks").cloned().unwrap_or_else(|| json!({}));
    for p in &plugins {
        if let Some(h) = p.hooks() {
            forge_config::deep_merge(&mut hooks_value, &h);
        }
    }
    let (hooks_cfg, hook_errors) = HooksConfig::from_settings(Some(&hooks_value));
    warnings.extend(hook_errors.into_iter().map(|e| format!("hooks: {e}")));
    let mut hooks = HookRunner::new(
        hooks_cfg,
        HookBase {
            session_id: session_id.clone(),
            transcript_path: transcript.path().map(Path::to_path_buf),
            cwd: cwd.clone(),
            project_dir: cwd.clone(),
        },
    );
    hooks.disabled = opts.bare || settings.bool("/disableAllHooks") == Some(true);
    if let Some(agent) = &main_agent {
        if let Some(allowed) = &agent.tools {
            tools.retain(|n| allowed.iter().any(|a| a == n));
        }
    }

    // Model and system prompt.
    let model = forge_api::resolve_model(
        &opts
            .model
            .clone()
            .or_else(|| env_nonempty("FORGE_MODEL"))
            .or_else(|| settings.str("/model").map(str::to_string))
            .unwrap_or_else(|| "default".into()),
    );
    let fallback_models: Vec<String> = opts
        .fallback_models
        .iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(forge_api::resolve_model)
        .collect();
    let verify = verify_config(&settings, &cwd);
    let mut env_info = EnvInfo::collect(&cwd, &add_dirs, &model);
    if let Some(v) = &verify {
        env_info.checks = v.commands.clone();
    }
    let prompt = PromptSpec {
        replace: opts.system_prompt.clone(),
        append: opts.append_system_prompt.clone(),
        agent: main_agent.as_ref().map(|a| a.prompt.clone()),
        mcp_instructions: opts.mcp.as_ref().and_then(|m| m.instructions()),
        prompts_dir: env_nonempty("FORGE_PROMPTS_DIR").map(PathBuf::from),
        exclude_dynamic: opts.exclude_dynamic_system_prompt_sections,
        output_style: Some(style.prompt.clone()).filter(|p| !p.is_empty()),
        env: env_info,
    };
    let sp_opts = prompt.options();
    for (tool, text) in forge_engine::prompts::tool_description_overrides(&sp_opts) {
        tools.set_description(&tool, text);
    }
    let (system, deferred) = prompt.build();
    let mut initial = vec![];
    if let Some(d) = deferred {
        initial.push(d);
    }
    if !opts.bare {
        if let Some(m) = memory_context(&cwd) {
            initial.push(m);
        }
    }

    // Sub-agents: the Task tool shares this session's provider, rules, hooks and budget.
    let memory = if opts.bare { None } else { memory_context(&cwd) };
    if tools.get("Task").is_none()
        && opts.tools.as_ref().map(|t| t.iter().any(|x| x.contains("Task") || x == "default")).unwrap_or(true)
        && main_agent.as_ref().and_then(|a| a.tools.as_ref()).map(|t| t.iter().any(|x| x == "Task")).unwrap_or(true)
        && !removed.iter().any(|r| r == "Task")
    {
        let rt = Arc::new(forge_agents::AgentRuntime {
            provider: provider.clone(),
            agents: agents.clone(),
            project_dir: cwd.clone(),
            working_dirs: tool_ctx.working_dirs.clone(),
            env: tool_ctx.env.clone(),
            sandbox: tool_ctx.sandbox.clone(),
            extra_tools: web_tools
                .iter()
                .cloned()
                .chain(opts.mcp.as_ref().map(|m| m.tools()).unwrap_or_default())
                .collect(),
            store: agent_store,
            session_id: session_id.clone(),
            hooks: hooks.clone(),
            sink: sink.clone(),
            base: EngineConfig {
                max_output_tokens: env_nonempty("FORGE_MAX_OUTPUT_TOKENS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(32_000),
                pricing: pricing_from_settings(&settings),
                ..Default::default()
            },
            memory_context: memory.clone(),
            parent: std::sync::OnceLock::new(),
        });
        tools.register(Arc::new(forge_agents::TaskTool { rt: rt.clone() }));
        let _ = agent_rt_slot.set(rt);
    }
    let tool_names = tools.names();
    let cfg = EngineConfig {
        model: model.clone(),
        fallback_models,
        max_output_tokens: env_nonempty("FORGE_MAX_OUTPUT_TOKENS").and_then(|v| v.parse().ok()).unwrap_or(32_000),
        effort: opts.effort.clone().or_else(|| settings.str("/effortLevel").map(str::to_string)),
        max_thinking_tokens: env_nonempty("FORGE_MAX_THINKING_TOKENS")
            .and_then(|v| v.parse().ok())
            .or_else(|| (settings.bool("/alwaysThinkingEnabled") == Some(false)).then_some(0)),
        max_turns: opts.max_turns,
        max_budget_usd: opts.max_budget_usd,
        json_schema: opts.json_schema.clone(),
        pricing: pricing_from_settings(&settings),
        initial_context: (!initial.is_empty()).then(|| initial.join("\n\n")),
        metadata_user_id: None,
        autocompact_window: match opts.autocompact.as_deref() {
            Some(v) => parse_autocompact(v)?,
            None => settings.get("/autoCompactWindow").and_then(Value::as_u64),
        },
        auto_compact: settings.bool("/autoCompactEnabled").unwrap_or(true),
        is_subagent: false,
        verify,
        escalate_output: env_nonempty("FORGE_MAX_OUTPUT_TOKENS").is_none(),
        fast: settings.bool("/fastMode").unwrap_or(false),
    };
    let parts = EngineParts {
        provider: provider.clone(),
        tools,
        tool_ctx,
        permissions,
        hooks,
        prompter,
        sink,
        transcript: transcript.clone(),
        history: Arc::new(match &opts.store_root {
            Some(root) => FileHistory::new(root.with_file_name("file-history").join(&session_id)),
            None => FileHistory::for_session(&session_id),
        }),
        system,
    };
    let mut engine = Engine::new(cfg, parts)?;
    if let Some(rt) = agent_rt_slot.get() {
        let _ = rt.parent.set(forge_agents::ParentLink {
            handle: engine.handle(),
            prompter: engine.prompter(),
            history: engine.history().clone(),
        });
    }
    if let Some(r) = resumed.as_mut() {
        engine.restore(r);
    }

    // Persist "always allow" answers to the settings file they name.
    let cwd_for_updates = cwd.clone();
    engine.set_permission_update_handler(Box::new(move |upd| persist_permission_update(&cwd_for_updates, upd)));

    let init = InitInfo {
        cwd: cwd.display().to_string(),
        session_id: session_id.clone(),
        tools: tool_names,
        mcp_servers: opts
            .mcp
            .as_ref()
            .map(|m| m.status().into_iter().map(|(name, status)| McpServerStatus { name, status }).collect())
            .unwrap_or_default(),
        model,
        permission_mode: mode.as_str().into(),
        slash_commands: commands::Catalog {
            commands: commands.clone(),
            skills: skills.clone(),
            mcp: opts.mcp.clone(),
            ..Default::default()
        }
        .names(commands::Surface::Stream),
        api_key_source: if opts.provider.is_some() { "none".into() } else { key_source(&settings) },
        forge_version: VERSION.into(),
        output_style: style.name.clone(),
        agents: agents.iter().map(|a| a.name.clone()).collect(),
        skills: skills.iter().map(|s| s.name.clone()).collect(),
        plugins: plugins.iter().map(|p| json!({"name": p.name, "path": p.dir})).collect(),
        uuid: uuid::Uuid::new_v4().to_string(),
    };
    Ok(Session {
        engine,
        init,
        prompt,
        commands,
        skills,
        styles,
        plugins,
        agents,
        settings,
        warnings,
        session_id,
        resumed,
    })
}

/// `auto` or a token count between 100k and 1M (`200000`, `200k`, `1m`).
pub fn parse_autocompact(v: &str) -> Result<Option<u64>, CoreError> {
    let t = v.trim().to_ascii_lowercase();
    if t == "auto" {
        return Ok(None);
    }
    let n = if let Some(k) = t.strip_suffix('k') {
        k.parse::<f64>().ok().map(|n| (n * 1_000.0) as u64)
    } else if let Some(m) = t.strip_suffix('m') {
        m.parse::<f64>().ok().map(|n| (n * 1_000_000.0) as u64)
    } else {
        t.replace('_', "").parse::<u64>().ok()
    };
    match n {
        Some(n) if (100_000..=1_000_000).contains(&n) => Ok(Some(n)),
        _ => Err(CoreError::Config(format!("--autocompact must be auto or 100k-1M tokens, got {v:?}"))),
    }
}

fn key_source(settings: &LoadedSettings) -> String {
    let c = MessagesConfig::from_env();
    if env_nonempty("FORGE_OPENAI_BASE_URL").is_some() {
        "FORGE_OPENAI_API_KEY".into()
    } else if c.has_credentials() {
        c.key_source().into()
    } else if settings.str("/apiKeyHelper").is_some() {
        "apiKeyHelper".into()
    } else {
        "none".into()
    }
}

/// Write an accepted `addRules` / `setMode` / `addDirectories` update to its settings file.
pub fn persist_permission_update(cwd: &Path, upd: &Value) {
    let dest = upd.get("destination").and_then(Value::as_str).unwrap_or("session");
    let file = match dest {
        "userSettings" => forge_config::forge_home().join("settings.json"),
        "projectSettings" => cwd.join(".forge/settings.json"),
        "localSettings" => cwd.join(".forge/settings.local.json"),
        _ => return,
    };
    let current: Value =
        std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
    let result = match upd.get("type").and_then(Value::as_str) {
        Some("addRules") => {
            let behavior = upd.get("behavior").and_then(Value::as_str).unwrap_or("allow");
            let mut list: Vec<Value> = current
                .pointer(&format!("/permissions/{behavior}"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for r in upd.get("rules").and_then(Value::as_array).into_iter().flatten() {
                if let Ok(rv) = serde_json::from_value::<forge_permissions::RuleValue>(r.clone()) {
                    let s = json!(rv.to_rule_string());
                    if !list.contains(&s) {
                        list.push(s);
                    }
                }
            }
            forge_config::write_setting(&file, &["permissions", behavior], Value::Array(list))
        }
        Some("setMode") => forge_config::write_setting(&file, &["permissions", "defaultMode"], upd["mode"].clone()),
        Some("addDirectories") => {
            let mut list: Vec<Value> = current
                .pointer("/permissions/additionalDirectories")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for d in upd.get("directories").and_then(Value::as_array).into_iter().flatten() {
                if !list.contains(d) {
                    list.push(d.clone());
                }
            }
            forge_config::write_setting(&file, &["permissions", "additionalDirectories"], Value::Array(list))
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, file = %file.display(), "could not save permission update");
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod driver_tests;
