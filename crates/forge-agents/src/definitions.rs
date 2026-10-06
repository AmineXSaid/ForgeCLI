use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSource {
    Builtin,
    User,
    Project,
    Flag,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDef {
    pub name: String,
    /// When to use it (shown to the model in the Task tool description).
    pub description: String,
    /// The agent's system prompt.
    pub prompt: String,
    /// Allowed tools; `None` = every tool except Task.
    pub tools: Option<Vec<String>>,
    /// Model alias or id; `None` / `inherit` = the caller's model.
    pub model: Option<String>,
    pub source: AgentSource,
}

const READ_ONLY_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch"];

const GENERAL: &str = "You are a sub-agent working for Forge, an agentic coding assistant. You were given one \
task by the main agent, which cannot see your work, only your final message.\n\n\
- Do the whole task with the tools available. Search broadly when you do not know where something lives; \
read the relevant code before drawing conclusions.\n\
- Do not ask questions: nobody can answer them. Make reasonable assumptions and state them.\n\
- Do not create files unless the task requires it.\n\
- Finish with a concise report: the answer or outcome first, then the evidence (file paths with line numbers, \
commands run and their results), then anything unresolved. That report is all the caller receives.";

const EXPLORE: &str = "You are a fast, read-only search sub-agent working for Forge. You find things in a \
codebase and report them precisely. You cannot modify files.\n\n\
- Start broad (Glob for names, Grep for contents), narrow quickly, and read only what you need.\n\
- Run independent searches in parallel in a single response.\n\
- Match the thoroughness the caller asked for: \"quick\" (one or two targeted searches), \"medium\" (the obvious \
places plus related code), or \"very thorough\" (several naming conventions and locations).\n\
- Report findings as a short list with `path:line` references and one line on why each matters. Say clearly if \
something was not found and where you looked.";

const PLAN: &str = "You are a read-only planning sub-agent working for Forge. You study the codebase and design \
an implementation plan; you do not change anything.\n\n\
- Read the code the change touches and find existing functions, patterns and tests to reuse.\n\
- Produce: the approach in a few sentences; the ordered steps; each file to change and what changes in it; \
risks and open questions; and how to verify the result (which tests or commands to run).\n\
- Prefer the smallest change that fully solves the problem.";

pub fn builtin_agents() -> Vec<AgentDef> {
    let ro = || Some(READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    vec![
        AgentDef {
            name: "general-purpose".into(),
            description: "Researches questions and carries out multi-step tasks with every tool. Use it for \
                          open-ended searches you might not get right first time, or self-contained work you \
                          want done in a separate context."
                .into(),
            prompt: GENERAL.into(),
            tools: None,
            model: None,
            source: AgentSource::Builtin,
        },
        AgentDef {
            name: "Explore".into(),
            description: "Fast read-only codebase search. Use it to find files, symbols or behaviour across many \
                          files; say how thorough to be (quick, medium, very thorough)."
                .into(),
            prompt: EXPLORE.into(),
            tools: ro(),
            model: None,
            source: AgentSource::Builtin,
        },
        AgentDef {
            name: "Plan".into(),
            description: "Read-only design of an implementation plan: steps, files to change, risks and how to \
                          verify. Use it before large or unfamiliar changes."
                .into(),
            prompt: PLAN.into(),
            tools: ro(),
            model: None,
            source: AgentSource::Builtin,
        },
    ]
}

fn parse_list(raw: &str) -> Vec<String> {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse an agent file: `---` frontmatter (`name`, `description`, `tools`,
/// `model`) followed by the system prompt.
pub fn parse_agent_markdown(text: &str, source: AgentSource) -> Result<AgentDef, String> {
    let rest = text.strip_prefix("---").ok_or("missing --- frontmatter")?;
    let end = rest.find("\n---").ok_or("unterminated frontmatter")?;
    let (front, body) = (&rest[..end], &rest[end + 4..]);
    let mut def = AgentDef {
        name: String::new(),
        description: String::new(),
        prompt: body.trim_start_matches(['\r', '\n']).trim_end().to_string(),
        tools: None,
        model: None,
        source,
    };
    let mut list_key: Option<String> = None;
    let mut list: Vec<String> = vec![];
    for line in front.lines() {
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            if list_key.is_some() {
                list.push(item.trim().trim_matches(['"', '\'']).to_string());
            }
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim().trim_matches(['"', '\'']));
        if list_key.take().as_deref() == Some("tools") {
            def.tools = Some(std::mem::take(&mut list));
        }
        match k {
            "name" => def.name = v.to_string(),
            "description" => def.description = v.to_string(),
            "model" => def.model = Some(v.to_string()).filter(|m| !m.is_empty() && m != "inherit"),
            "tools" if v.is_empty() => list_key = Some("tools".into()),
            "tools" => def.tools = Some(parse_list(v)),
            _ => {}
        }
    }
    if list_key.as_deref() == Some("tools") {
        def.tools = Some(list);
    }
    if def.name.is_empty() {
        return Err("frontmatter needs a name".into());
    }
    if def.description.is_empty() {
        return Err(format!("agent {} needs a description", def.name));
    }
    Ok(def)
}

/// `--agents '{"reviewer": {"description": "...", "prompt": "...", "tools": [...], "model": "..."}}'`
pub fn parse_agents_json(raw: &str) -> Result<Vec<AgentDef>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("--agents: {e}"))?;
    let obj = v.as_object().ok_or("--agents must be a JSON object")?;
    obj.iter()
        .map(|(name, a)| {
            let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
            Ok(AgentDef {
                name: name.clone(),
                description: s("description").ok_or(format!("--agents {name}: description is required"))?,
                prompt: s("prompt").ok_or(format!("--agents {name}: prompt is required"))?,
                tools: a
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(|t| t.iter().filter_map(Value::as_str).map(str::to_string).collect()),
                model: s("model").filter(|m| m != "inherit"),
                source: AgentSource::Flag,
            })
        })
        .collect()
}

/// Built-ins, then user, project and flag agents; later definitions replace earlier ones by name.
pub fn load_agents(project: &Path, flag: &[AgentDef], warnings: &mut Vec<String>) -> Vec<AgentDef> {
    let mut out = builtin_agents();
    for (scope, dir) in forge_config::resource_dirs("agents", project) {
        let source = match scope {
            forge_config::Scope::User => AgentSource::User,
            forge_config::Scope::Project => AgentSource::Project,
        };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> =
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
        files.sort();
        for f in files {
            match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| parse_agent_markdown(&t, source))
            {
                Ok(def) => {
                    out.retain(|d| d.name != def.name);
                    out.push(def);
                }
                Err(e) => warnings.push(format!("agent {}: {e}", f.display())),
            }
        }
    }
    for def in flag {
        out.retain(|d| d.name != def.name);
        out.push(def.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_agents() {
        let a = parse_agent_markdown(
            "---\nname: reviewer\ndescription: Reviews diffs\ntools: Read, Grep\nmodel: sonnet\n---\nYou review code.\n",
            AgentSource::Project,
        )
        .unwrap();
        assert_eq!(a.name, "reviewer");
        assert_eq!(a.tools, Some(vec!["Read".into(), "Grep".into()]));
        assert_eq!(a.model.as_deref(), Some("sonnet"));
        assert_eq!(a.prompt, "You review code.");
        let b = parse_agent_markdown(
            "---\nname: x\ndescription: d\ntools:\n  - Read\n  - Bash\nmodel: inherit\n---\nP",
            AgentSource::User,
        )
        .unwrap();
        assert_eq!(b.tools, Some(vec!["Read".into(), "Bash".into()]));
        assert_eq!(b.model, None);
        assert!(parse_agent_markdown("no frontmatter", AgentSource::User).is_err());
        assert!(parse_agent_markdown("---\nname: x\n---\nP", AgentSource::User).is_err());
    }

    #[test]
    fn json_agents_and_precedence() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".forge/agents")).unwrap();
        std::fs::write(
            d.path().join(".forge/agents/Explore.md"),
            "---\nname: Explore\ndescription: custom explore\n---\nMine",
        )
        .unwrap();
        std::fs::write(d.path().join(".forge/agents/bad.md"), "oops").unwrap();
        let flag =
            parse_agents_json(r#"{"reviewer": {"description": "Reviews", "prompt": "Review it", "tools": ["Read"]}}"#)
                .unwrap();
        let mut warnings = vec![];
        let all = load_agents(d.path(), &flag, &mut warnings);
        assert_eq!(warnings.len(), 1);
        let explore = all.iter().find(|a| a.name == "Explore").unwrap();
        assert_eq!(explore.description, "custom explore");
        assert_eq!(explore.source, AgentSource::Project);
        assert!(all.iter().any(|a| a.name == "reviewer" && a.source == AgentSource::Flag));
        assert!(parse_agents_json(r#"{"x": {"prompt": "p"}}"#).is_err());
    }
}
