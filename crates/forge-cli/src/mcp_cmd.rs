//! `forge mcp`: add, remove, list, inspect and approve MCP servers, and serve
//! Forge's own tools over MCP.

use std::collections::BTreeMap;
use std::path::PathBuf;

use forge_mcp::config::{self, ServerConfig};
use serde_json::{json, Value};

use crate::args::{McpAction, Opts};
use crate::exit::{self, Fail};
use crate::term;

/// The file a scope writes to.
fn scope_file(scope: &str, cwd: &std::path::Path) -> Result<PathBuf, Fail> {
    match scope {
        "local" => Ok(cwd.join(".forge/settings.local.json")),
        "project" => Ok(config::project_file(cwd)),
        "user" => Ok(forge_config::config_dir().join("settings.json")),
        other => Err(Fail::usage(format!("invalid --scope {other:?}: expected local, project or user"))),
    }
}

fn parse_pairs(items: &[String], sep: char, what: &str) -> Result<BTreeMap<String, String>, Fail> {
    items
        .iter()
        .map(|i| match i.split_once(sep) {
            Some((k, v)) if !k.trim().is_empty() => Ok((k.trim().to_string(), v.trim().to_string())),
            _ => Err(Fail::usage(format!("invalid {what} {i:?}: expected KEY{sep}value"))),
        })
        .collect()
}

fn check_name(name: &str) -> Result<(), Fail> {
    if config::valid_name(name) {
        Ok(())
    } else {
        Err(Fail::usage(format!("invalid server name {name:?}: use letters, digits, '-' and '_'")))
    }
}

/// A config with secret values hidden, for printing.
fn redacted(c: &ServerConfig) -> Value {
    let mut v = c.to_json();
    for key in ["env", "headers"] {
        if let Some(m) = v.get_mut(key).and_then(Value::as_object_mut) {
            for val in m.values_mut() {
                *val = json!("***");
            }
        }
    }
    v
}

pub async fn run(action: McpAction, opts: &Opts) -> Result<i32, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(format!("cannot read the current directory: {e}")))?;
    match action {
        McpAction::Serve => serve(&cwd).await,
        McpAction::Add { scope, transport, env, header, name, command_or_url, args } => {
            check_name(&name)?;
            let is_url = command_or_url.starts_with("http://") || command_or_url.starts_with("https://");
            let kind = transport.unwrap_or_else(|| if is_url { "http".into() } else { "stdio".into() });
            let cfg = match kind.as_str() {
                "stdio" => {
                    if !header.is_empty() {
                        return Err(Fail::usage("--header is for http and sse servers"));
                    }
                    ServerConfig::Stdio { command: command_or_url, args, env: parse_pairs(&env, '=', "--env")? }
                }
                "http" | "sse" => {
                    if !is_url {
                        return Err(Fail::usage(format!("{kind} servers need a URL, got {command_or_url:?}")));
                    }
                    if !args.is_empty() || !env.is_empty() {
                        return Err(Fail::usage("arguments and --env are for stdio servers"));
                    }
                    let headers = parse_pairs(&header, ':', "--header")?;
                    if kind == "http" {
                        ServerConfig::Http { url: command_or_url, headers }
                    } else {
                        ServerConfig::Sse { url: command_or_url, headers }
                    }
                }
                other => {
                    return Err(Fail::usage(format!("invalid --transport {other:?}: expected stdio, http or sse")))
                }
            };
            add(&cwd, &scope, &name, &cfg)
        }
        McpAction::AddJson { scope, name, json } => {
            check_name(&name)?;
            let v: Value = serde_json::from_str(&json).map_err(|e| Fail::usage(format!("not valid JSON: {e}")))?;
            let cfg = config::parse_server(&v).map_err(Fail::usage)?;
            add(&cwd, &scope, &name, &cfg)
        }
        McpAction::Remove { scope, name } => {
            let scopes: Vec<String> = match scope {
                Some(s) => vec![s],
                None => vec!["local".into(), "project".into(), "user".into()],
            };
            let mut removed = vec![];
            for s in &scopes {
                let file = scope_file(s, &cwd)?;
                if file.exists() && config::write_server(&file, &name, None).map_err(|e| Fail::config(e.to_string()))? {
                    removed.push(format!("{s} ({})", file.display()));
                }
            }
            if removed.is_empty() {
                eprintln!("forge: no MCP server named {name} in {} scope", scopes.join(", "));
                return Ok(exit::FAILED);
            }
            println!("Removed MCP server {name} from {}", removed.join(" and "));
            Ok(exit::OK)
        }
        McpAction::List => list(&cwd, opts).await,
        McpAction::Get { name } => get(&cwd, opts, &name).await,
        McpAction::Approve { name, all } => approve(&cwd, name, all),
    }
}

fn add(cwd: &std::path::Path, scope: &str, name: &str, cfg: &ServerConfig) -> Result<i32, Fail> {
    let file = scope_file(scope, cwd)?;
    let replaced = config::write_server(&file, name, Some(cfg)).map_err(|e| Fail::config(e.to_string()))?;
    println!(
        "{} MCP server {name} ({}) in {scope} scope: {}",
        if replaced { "Updated" } else { "Added" },
        cfg.transport(),
        file.display()
    );
    if scope == "project" {
        println!("It is shared through .mcp.json; each person trusts it with `forge mcp approve {name}`.");
        // The person adding it trusts it.
        approve(cwd, Some(name.to_string()), false)?;
    }
    Ok(exit::OK)
}

fn launch(cwd: &std::path::Path, opts: &Opts) -> forge_core::LaunchOptions {
    forge_core::LaunchOptions {
        cwd: cwd.to_path_buf(),
        settings: opts.settings.clone(),
        mcp_configs: opts.mcp_config.clone(),
        strict_mcp_config: opts.strict_mcp_config,
        ..Default::default()
    }
}

async fn list(cwd: &std::path::Path, opts: &Opts) -> Result<i32, Fail> {
    let lo = launch(cwd, opts);
    let resolved = forge_core::resolve_mcp(&lo);
    for w in &resolved.warnings {
        eprintln!("{} {w}", term::yellow("forge: warning:"));
    }
    if resolved.servers.is_empty() && resolved.skipped.is_empty() {
        println!("No MCP servers configured. Add one with `forge mcp add`.");
        return Ok(exit::OK);
    }
    println!("Checking MCP server health...\n");
    let m = forge_mcp::McpManager::connect(&resolved, &forge_mcp::ConnectOptions::new(cwd)).await;
    let mut failed = false;
    for s in &m.servers {
        let summary = resolved
            .servers
            .iter()
            .find(|n| n.name == s.name)
            .map(|n| n.config.summary())
            .unwrap_or_else(|| ".mcp.json".into());
        let status = match &s.status {
            forge_mcp::Status::Connected => term::green(&format!("connected, {} tools", s.tools.len())),
            forge_mcp::Status::Failed(e) => {
                failed = true;
                term::red(&format!("failed: {e}"))
            }
            forge_mcp::Status::Skipped(r) => term::yellow(r),
        };
        println!("{}: {summary} - {status}", s.name);
    }
    m.shutdown().await;
    Ok(if failed { exit::FAILED } else { exit::OK })
}

async fn get(cwd: &std::path::Path, opts: &Opts, name: &str) -> Result<i32, Fail> {
    let lo = launch(cwd, opts);
    let mut resolved = forge_core::resolve_mcp(&lo);
    if let Some(s) = resolved.skipped.iter().find(|s| s.name == name) {
        println!("{name}:\n  Scope: project (.mcp.json)\n  Status: {}", s.reason);
        return Ok(exit::OK);
    }
    let Some(server) = resolved.servers.iter().find(|s| s.name == name).cloned() else {
        eprintln!("forge: no MCP server named {name}");
        return Ok(exit::FAILED);
    };
    resolved.servers.retain(|s| s.name == name);
    let m = forge_mcp::McpManager::connect(&resolved, &forge_mcp::ConnectOptions::new(cwd)).await;
    let entry = &m.servers[0];
    println!("{name}:");
    println!("  Scope: {}", server.scope.as_str());
    println!("  Config: {}", serde_json::to_string(&redacted(&server.config)).unwrap_or_default());
    match &entry.status {
        forge_mcp::Status::Connected => {
            println!("  Status: connected");
            if let Some(c) = &entry.client {
                println!("  Server: {}", c.server_info);
                println!("  Protocol: {}", c.protocol_version);
            }
            println!("  Tools ({}):", entry.tools.len());
            for t in &entry.tools {
                println!(
                    "    {} - {}",
                    forge_mcp::tools::tool_name(name, &t.name),
                    t.description.lines().next().unwrap_or("")
                );
            }
        }
        forge_mcp::Status::Failed(e) => println!("  Status: failed: {e}"),
        forge_mcp::Status::Skipped(r) => println!("  Status: {r}"),
    }
    m.shutdown().await;
    Ok(exit::OK)
}

fn approve(cwd: &std::path::Path, name: Option<String>, all: bool) -> Result<i32, Fail> {
    let local = cwd.join(".forge/settings.local.json");
    let write =
        |ptr: &[&str], v: Value| forge_config::write_setting(&local, ptr, v).map_err(|e| Fail::config(e.to_string()));
    if all {
        write(&["enableAllProjectMcpServers"], json!(true))?;
        println!("Trusting every server in this project's .mcp.json ({}).", local.display());
        return Ok(exit::OK);
    }
    let Some(name) = name else {
        return Err(Fail::usage("name a server to approve, or pass --all"));
    };
    let current: Value =
        std::fs::read_to_string(&local).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let mut list: Vec<Value> =
        current.get("enabledMcpjsonServers").and_then(Value::as_array).cloned().unwrap_or_default();
    if !list.iter().any(|v| v.as_str() == Some(&name)) {
        list.push(json!(name));
    }
    let disabled: Vec<Value> = current
        .get("disabledMcpjsonServers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|v| v.as_str() != Some(&name)).cloned().collect())
        .unwrap_or_default();
    write(&["enabledMcpjsonServers"], json!(list))?;
    write(&["disabledMcpjsonServers"], json!(disabled))?;
    println!("Trusting {name} from this project's .mcp.json ({}).", local.display());
    Ok(exit::OK)
}

async fn serve(cwd: &std::path::Path) -> Result<i32, Fail> {
    let mut reg = forge_tools::ToolRegistry::new();
    forge_tools::builtin::register_core(&mut reg);
    let ctx = forge_tools::ToolContext::new(&cwd.canonicalize().unwrap_or(cwd.to_path_buf()));
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    forge_mcp::server::serve(reg, ctx, stdin, tokio::io::stdout())
        .await
        .map_err(|e| Fail::config(format!("mcp serve: {e}")))?;
    Ok(exit::OK)
}
