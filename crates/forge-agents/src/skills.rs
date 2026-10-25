//! Skills: `skills/<name>/SKILL.md` folders of instructions (and any files
//! they reference), loaded on demand.
//!
//! Only each skill's name and description go in front of the model, in the
//! Skill tool's description. The body enters the context when the model
//! invokes the skill, or when the user types `/<name>`. This keeps the
//! context small (GOALS pillar 1).

use std::path::{Path, PathBuf};

use forge_permissions::Subject;
use forge_tools::{Tool, ToolContext, ToolOutput};
use serde_json::{json, Value};

use crate::frontmatter;
use crate::substitute::SkillVars;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub body: String,
    /// `disable-model-invocation: true` keeps it out of the Skill tool (user `/name` only).
    pub model_invocable: bool,
    /// `user-invocable: false` hides it from `/` (model only).
    pub user_invocable: bool,
    pub source: String,
    /// `arguments`: names for its positional arguments (`$issue`).
    pub arguments: Vec<String>,
    /// `argument-hint`: what to type after `/name`.
    pub argument_hint: Option<String>,
    /// `allowed-tools`: rules allowed without asking while it runs.
    pub allowed_tools: Vec<String>,
    /// `disallowed-tools`: tools taken away while it runs.
    pub disallowed_tools: Vec<String>,
    /// `model` and `effort` for the turn it runs in (`inherit` keeps the session's).
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `context: fork`: it runs as a sub-agent of type `agent`.
    pub fork: bool,
    pub agent: Option<String>,
    /// `background`: with `fork`, whether the caller goes on without waiting.
    pub background: Option<bool>,
    /// `paths`: it is offered to the model only once a matching file is in play.
    pub paths: Vec<String>,
    /// `shell`: `bash` or `powershell`, for its ``!`command` `` lines.
    pub shell: Option<String>,
    /// The plugin it came with: the plugin's folder.
    pub plugin_dir: Option<PathBuf>,
}

/// Descriptions longer than this are cut in the Skill tool's list.
const DESCRIPTION_MAX: usize = 1536;

pub fn parse_skill(dir: &Path, text: &str, source: &str) -> Result<SkillDef, String> {
    let (fm, body) = frontmatter::parse(text);
    let name = fm
        .text("name")
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))
        .ok_or("no name")?;
    // No description: the body's first line says what it is, as the reference CLI reads it.
    let mut description = fm
        .text("description")
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .or_else(|| {
            body.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty()).map(str::to_string)
        })
        .ok_or_else(|| format!("skill {name} needs a description"))?;
    if let Some(when) = fm.text("when_to_use").filter(|w| !w.is_empty()) {
        description = format!("{description} {when}");
    }
    if description.chars().count() > DESCRIPTION_MAX {
        description = description.chars().take(DESCRIPTION_MAX - 1).collect::<String>() + "…";
    }
    let text = |k: &str| fm.text(k).filter(|v| !v.is_empty()).map(str::to_string);
    let list = |k: &str| fm.list(k).unwrap_or_default();
    Ok(SkillDef {
        name,
        description,
        dir: dir.to_path_buf(),
        body,
        model_invocable: !fm.flag("disable-model-invocation").unwrap_or(false),
        user_invocable: fm.flag("user-invocable").unwrap_or(true),
        source: source.to_string(),
        arguments: list("arguments"),
        argument_hint: text("argument-hint"),
        allowed_tools: list("allowed-tools"),
        disallowed_tools: list("disallowed-tools"),
        model: text("model").filter(|m| m != "inherit"),
        effort: text("effort"),
        fork: fm.text("context") == Some("fork"),
        agent: text("agent"),
        background: fm.flag("background"),
        paths: list("paths"),
        shell: text("shell"),
        plugin_dir: None,
    })
}

/// Skills that ship with Forge (Forge's own text). They have the lowest
/// precedence: a user, project or plugin skill with the same name replaces one.
const BUNDLED: &[(&str, &str)] = &[
    ("batch", include_str!("../bundled/batch.md")),
    ("code-review", include_str!("../bundled/code-review.md")),
    ("fewer-permission-prompts", include_str!("../bundled/fewer-permission-prompts.md")),
    ("init", include_str!("../bundled/init.md")),
    ("run", include_str!("../bundled/run.md")),
    ("run-skill-generator", include_str!("../bundled/run-skill-generator.md")),
    ("security-review", include_str!("../bundled/security-review.md")),
    ("simplify", include_str!("../bundled/simplify.md")),
    ("update-config", include_str!("../bundled/update-config.md")),
    ("verify", include_str!("../bundled/verify.md")),
];

/// The bundled skills. `FORGE_PROMPTS_DIR/skills/<name>.md` replaces one's text.
/// `/review` is another name for `/code-review` (for people only).
pub fn bundled_skills(warnings: &mut Vec<String>) -> Vec<SkillDef> {
    let overrides = std::env::var_os("FORGE_PROMPTS_DIR").map(|d| PathBuf::from(d).join("skills"));
    let mut out = vec![];
    for (name, text) in BUNDLED {
        let custom = overrides.as_ref().and_then(|d| std::fs::read_to_string(d.join(format!("{name}.md"))).ok());
        let text = custom.as_deref().unwrap_or(text);
        match parse_skill(Path::new(""), text, "bundled") {
            Ok(mut s) => {
                s.name = name.to_string();
                out.push(s);
            }
            Err(e) => warnings.push(format!("bundled skill {name}: {e}")),
        }
    }
    if let Some(review) = out.iter().find(|s| s.name == "code-review").cloned() {
        out.push(SkillDef {
            name: "review".into(),
            description: format!("{} (same as /code-review)", review.description),
            model_invocable: false,
            ..review
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Skills from user, project (`.agents/`, `.forge/`) and plugin `skills/` directories.
pub fn load_skills(project: &Path, plugins: &[crate::plugins::Plugin], warnings: &mut Vec<String>) -> Vec<SkillDef> {
    let mut dirs: Vec<(PathBuf, String, Option<String>)> = forge_config::resource_dirs("skills", project)
        .into_iter()
        .map(|(scope, d)| (d, if scope == forge_config::Scope::User { "user" } else { "project" }.to_string(), None))
        .collect();
    for p in plugins {
        dirs.push((p.dir.join("skills"), p.name.clone(), Some(p.name.clone())));
    }
    let plugin_dirs: Vec<(String, PathBuf)> = plugins.iter().map(|p| (p.name.clone(), p.dir.clone())).collect();
    let mut out: Vec<SkillDef> = bundled_skills(warnings);
    for (root, source, prefix) in dirs {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut subdirs: Vec<PathBuf> =
            rd.flatten().map(|e| e.path()).filter(|p| p.join("SKILL.md").is_file()).collect();
        subdirs.sort();
        for d in subdirs {
            match std::fs::read_to_string(d.join("SKILL.md"))
                .map_err(|e| e.to_string())
                .and_then(|t| parse_skill(&d, &t, &source))
            {
                Ok(mut s) => {
                    if let Some(p) = &prefix {
                        s.name = format!("{p}:{}", s.name);
                        s.plugin_dir = plugin_dirs.iter().find(|(n, _)| n == p).map(|(_, d)| d.clone());
                    }
                    out.retain(|o| o.name != s.name);
                    out.push(s);
                }
                Err(e) => warnings.push(format!("skill {}: {e}", d.display())),
            }
        }
    }
    out
}

/// The text a skill puts in the conversation: its body with its arguments
/// and folders filled in (`substitute.rs`).
pub fn skill_prompt(skill: &SkillDef, args: &str, ctx: &ToolContext) -> String {
    let plugin = skill.plugin_dir.as_deref().zip(skill.name.split_once(':').map(|(p, _)| p));
    let vars = SkillVars {
        skill_dir: Some(skill.dir.as_path()).filter(|d| !d.as_os_str().is_empty()),
        project_dir: Some(ctx.project_dir.as_path()),
        session_id: Some(ctx.session_id.as_str()).filter(|s| !s.is_empty()),
        effort: ctx.env.get("FORGE_EFFORT").map(String::as_str),
        plugin,
    };
    let body = crate::substitute::fill(skill.body.trim(), args, &skill.arguments, &vars);
    if skill.dir.as_os_str().is_empty() {
        body
    } else {
        format!("Base directory for this skill: {}\n\n{body}", skill.dir.display())
    }
}

/// `Skill`: load a skill's instructions.
pub struct SkillTool {
    pub skills: Vec<SkillDef>,
}

impl SkillTool {
    fn offered(&self) -> impl Iterator<Item = &SkillDef> {
        self.skills.iter().filter(|s| s.model_invocable)
    }
}

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> String {
        let mut list = String::new();
        for s in self.offered() {
            let line = format!("- {}: {}\n", s.name, s.description.chars().take(300).collect::<String>());
            if list.len() + line.len() > 12_000 {
                list.push_str("- (more skills not listed)\n");
                break;
            }
            list.push_str(&line);
        }
        format!(
            "Load a skill: packaged instructions (and files) for a specific kind of task. When the task matches a \
             skill below, invoke it first and follow what it says; it takes precedence over your default approach. \
             Do not invoke a skill that is already loaded in this conversation.\n\nAvailable skills:\n{list}"
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {"type": "string", "description": "The skill's name"},
                "args": {"type": "string", "description": "Optional arguments for the skill"}
            },
            "required": ["skill"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_enabled(&self) -> bool {
        self.offered().next().is_some()
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Name(
            input.get("skill").and_then(Value::as_str).unwrap_or_default().trim_start_matches('/').to_string(),
        )
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let name = input.get("skill").and_then(Value::as_str).unwrap_or_default().trim_start_matches('/');
        let args = input.get("args").and_then(Value::as_str).unwrap_or_default();
        match self.offered().find(|s| s.name == name || s.name.eq_ignore_ascii_case(name)) {
            Some(s) => {
                ToolOutput::text(skill_prompt(s, args, ctx)).with_structured(json!({"skill": s.name, "success": true}))
            }
            None => {
                let names: Vec<&str> = self.offered().map(|s| s.name.as_str()).collect();
                ToolOutput::error(format!("Unknown skill {name:?}. Available: {}", names.join(", ")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovers_and_loads_skills_on_demand() {
        let d = tempfile::tempdir().unwrap();
        let mk = |dir: &str, text: &str| {
            let p = d.path().join(dir);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("SKILL.md"), text).unwrap();
        };
        mk(".forge/skills/pdf", "---\nname: pdf\ndescription: Work with PDF files\n---\nUse pdftotext.");
        mk(
            ".agents/skills/deploy",
            "---\ndescription: Deploy the app\ndisable-model-invocation: true\n---\nRun make deploy.",
        );
        // No description: its first line stands in, as in the reference CLI.
        mk(".forge/skills/terse", "---\nname: terse\n---\n# Tidy imports\nSort them.");
        mk(".forge/skills/empty", "---\nname: empty\n---\n");
        // The reference CLI's project folder loads too.
        mk(".claude/skills/review-pr", "---\ndescription: Review a pull request\n---\nRead the diff.");
        let mut w = vec![];
        let skills = load_skills(d.path(), &[], &mut w);
        // (The user's own skills, if any on this machine, are left out of the check.)
        let own: Vec<&SkillDef> = skills.iter().filter(|s| s.dir.starts_with(d.path())).collect();
        let names: Vec<&str> = own.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["review-pr", "deploy", "pdf", "terse"]);
        assert_eq!(own[3].description, "Tidy imports");
        assert_eq!(w.iter().filter(|w| w.contains("empty")).count(), 1, "{w:?}");
        let tool = SkillTool { skills };
        assert!(tool.description().contains("- pdf: Work with PDF files") && !tool.description().contains("deploy"));
        let c = ToolContext::new(d.path());
        let out = tool.call(json!({"skill": "pdf", "args": "report.pdf"}), &c).await;
        assert!(out.text_content().contains("Use pdftotext.") && out.text_content().ends_with("ARGUMENTS: report.pdf"));
        assert!(out.text_content().starts_with("Base directory for this skill: "));
        assert!(tool.call(json!({"skill": "deploy"}), &c).await.is_error, "user-only skills are not model-invocable");
    }

    #[tokio::test]
    async fn reference_cli_skills_render_as_they_do_there() {
        let d = tempfile::tempdir().unwrap();
        let skill = d.path().join(".claude/skills/migrate");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\ndescription: Migrate a component\narguments: [component, from, to]\nallowed-tools: Bash(${CLAUDE_SKILL_DIR}/run.sh *) Read\ncontext: fork\nagent: Explore\nmodel: inherit\n---\nMigrate $component ($ARGUMENTS[0]) from $1 to $to.\nRun ${CLAUDE_SKILL_DIR}/run.sh in ${CLAUDE_PROJECT_DIR}.",
        )
        .unwrap();
        let plugin_dir = d.path().join("kit");
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(plugin_dir.join("skills/lint")).unwrap();
        std::fs::write(plugin_dir.join(".claude-plugin/plugin.json"), r#"{"name": "kit"}"#).unwrap();
        std::fs::write(
            plugin_dir.join("skills/lint/SKILL.md"),
            "---\ndescription: Lint\n---\nRun ${CLAUDE_PLUGIN_ROOT}/lint.",
        )
        .unwrap();
        let mut w = vec![];
        let plugins = crate::plugins::load_plugins(&[plugin_dir], &mut w);
        let skills = load_skills(d.path(), &plugins, &mut w);
        let m = skills.iter().find(|s| s.name == "migrate").unwrap();
        assert_eq!(m.allowed_tools, ["Bash(${CLAUDE_SKILL_DIR}/run.sh *)", "Read"]);
        assert!(m.fork && m.agent.as_deref() == Some("Explore") && m.model.is_none());
        let tool = SkillTool { skills: skills.clone() };
        let c = ToolContext::new(d.path());
        let out = tool.call(json!({"skill": "migrate", "args": "\"Button group\" react vue"}), &c).await;
        let text = out.text_content();
        assert!(text.contains("Migrate Button group (Button group) from react to vue."), "{text}");
        assert!(text.contains(&format!("Run {}/run.sh in {}.", skill.display(), d.path().display())), "{text}");
        assert!(!text.contains("ARGUMENTS:"), "the placeholders took the arguments");
        let lint = tool.call(json!({"skill": "kit:lint"}), &c).await.text_content();
        let root = plugins[0].dir.display().to_string();
        assert!(lint.ends_with(&format!("Run {root}/lint.")), "{lint}");
    }

    #[test]
    fn bundled_skills_load_and_give_way() {
        let mut w = vec![];
        let b = bundled_skills(&mut w);
        assert!(w.is_empty(), "{w:?}");
        let names: Vec<&str> = b.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "batch",
                "code-review",
                "fewer-permission-prompts",
                "init",
                "review",
                "run",
                "run-skill-generator",
                "security-review",
                "simplify",
                "update-config",
                "verify"
            ]
        );
        for s in &b {
            assert!(!s.description.is_empty() && s.description.len() < 110 && s.body.len() > 300, "{}", s.name);
            assert!(s.user_invocable);
            assert_eq!(s.source, "bundled");
        }
        // Only the ones a model should reach for on its own are offered to it.
        let offered: Vec<&str> = b.iter().filter(|s| s.model_invocable).map(|s| s.name.as_str()).collect();
        assert_eq!(offered, ["code-review", "security-review", "simplify", "update-config", "verify"]);
        let review = b.iter().find(|s| s.name == "review").unwrap();
        assert!(review.body.contains("--fix") && review.description.ends_with("(same as /code-review)"));
        // No directory line for bundled skills.
        assert!(skill_prompt(review, "--fix 12", &ToolContext::new(Path::new("/p"))).starts_with("Review code changes"));
        // Forge's own words only.
        for s in &b {
            let low = s.body.to_lowercase();
            assert!(!low.contains("claude") && !low.contains("anthropic"), "{}", s.name);
        }

        // A project skill with the same name replaces the bundled one.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".forge/skills/verify");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("SKILL.md"), "---\ndescription: Our way\n---\nRun make verify.").unwrap();
        let all = load_skills(d.path(), &[], &mut w);
        let v: Vec<&SkillDef> = all.iter().filter(|s| s.name == "verify").collect();
        assert_eq!((v.len(), v[0].description.as_str()), (1, "Our way"));
    }
}
