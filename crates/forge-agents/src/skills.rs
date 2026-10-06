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

#[derive(Debug, Clone, PartialEq)]
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
}

pub fn parse_skill(dir: &Path, text: &str, source: &str) -> Result<SkillDef, String> {
    let (fm, body) = frontmatter::parse(text);
    let name = fm
        .text("name")
        .map(str::to_string)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))
        .ok_or("no name")?;
    let description =
        fm.text("description").map(str::to_string).ok_or_else(|| format!("skill {name} needs a description"))?;
    Ok(SkillDef {
        name,
        description,
        dir: dir.to_path_buf(),
        body,
        model_invocable: !fm.flag("disable-model-invocation").unwrap_or(false),
        user_invocable: fm.flag("user-invocable").unwrap_or(true),
        source: source.to_string(),
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

/// The text a skill puts in the conversation.
pub fn skill_prompt(skill: &SkillDef, args: &str) -> String {
    let mut s = if skill.dir.as_os_str().is_empty() {
        skill.body.trim().to_string()
    } else {
        format!("Base directory for this skill: {}\n\n{}", skill.dir.display(), skill.body.trim())
    };
    if !args.trim().is_empty() {
        s.push_str(&format!("\n\nARGUMENTS: {}", args.trim()));
    }
    s
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

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let name = input.get("skill").and_then(Value::as_str).unwrap_or_default().trim_start_matches('/');
        let args = input.get("args").and_then(Value::as_str).unwrap_or_default();
        match self.offered().find(|s| s.name == name || s.name.eq_ignore_ascii_case(name)) {
            Some(s) => {
                ToolOutput::text(skill_prompt(s, args)).with_structured(json!({"skill": s.name, "success": true}))
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
        mk(".forge/skills/broken", "---\nname: broken\n---\nNo description.");
        let mut w = vec![];
        let skills = load_skills(d.path(), &[], &mut w);
        let own: Vec<&str> = skills.iter().filter(|s| s.source != "bundled").map(|s| s.name.as_str()).collect();
        assert_eq!(own, ["deploy", "pdf"]);
        assert_eq!(w.len(), 1, "{w:?}");
        let tool = SkillTool { skills };
        assert!(tool.description().contains("- pdf: Work with PDF files") && !tool.description().contains("deploy"));
        let c = ToolContext::new(d.path());
        let out = tool.call(json!({"skill": "pdf", "args": "report.pdf"}), &c).await;
        assert!(out.text_content().contains("Use pdftotext.") && out.text_content().ends_with("ARGUMENTS: report.pdf"));
        assert!(out.text_content().starts_with("Base directory for this skill: "));
        assert!(tool.call(json!({"skill": "deploy"}), &c).await.is_error, "user-only skills are not model-invocable");
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
        assert!(skill_prompt(review, "--fix 12").starts_with("Review code changes"));
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
