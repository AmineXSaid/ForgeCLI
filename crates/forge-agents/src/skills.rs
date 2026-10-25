//! Skills: `skills/<name>/SKILL.md` folders of instructions (and any files
//! they reference), loaded on demand.
//!
//! Only each skill's name and description go in front of the model, in the
//! Skill tool's description. The body enters the context when the model
//! invokes the skill, or when the user types `/<name>`. This keeps the
//! context small (GOALS pillar 1).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use forge_permissions::Subject;
use forge_tools::{Tool, ToolContext, ToolOutput};
use serde_json::{json, Value};

use crate::frontmatter;

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
    /// `hooks`: the settings `hooks` shape, kept for the session once it runs.
    pub hooks: Option<Value>,
    /// Its ``!`command` `` lines are not run (`disableSkillShellExecution`).
    pub no_shell: bool,
    /// Listed to the model by name only (`skillOverrides: name-only`).
    pub name_only: bool,
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
        hooks: fm.value("hooks").filter(Value::is_object),
        no_shell: false,
        name_only: false,
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
        for d in p.component_dirs("skills") {
            dirs.push((d, p.name.clone(), Some(p.name.clone())));
        }
    }
    let plugin_dirs: Vec<(String, PathBuf)> = plugins.iter().map(|p| (p.name.clone(), p.dir.clone())).collect();
    let mut out: Vec<SkillDef> = bundled_skills(warnings);
    // A plugin that is one skill: `SKILL.md` at its root, named by its `name` or the plugin's.
    for p in plugins {
        let file = p.dir.join("SKILL.md");
        if let Ok(text) = std::fs::read_to_string(&file) {
            match parse_skill(&p.dir, &text, &p.name) {
                Ok(mut s) => {
                    let has_name = frontmatter::parse(&text).0.text("name").is_some_and(|n| !n.is_empty());
                    s.name = if has_name { format!("{}:{}", p.name, s.name) } else { p.name.clone() };
                    s.plugin_dir = Some(p.dir.clone());
                    out.retain(|o| o.name != s.name);
                    out.push(s);
                }
                Err(e) => warnings.push(format!("skill {}: {e}", file.display())),
            }
        }
    }
    for (root, source, prefix) in dirs {
        for mut s in load_dir(&root, &source, warnings) {
            if let Some(p) = &prefix {
                s.name = format!("{p}:{}", s.name);
                s.plugin_dir = plugin_dirs.iter().find(|(n, _)| n == p).map(|(_, d)| d.clone());
            }
            out.retain(|o| o.name != s.name);
            out.push(s);
        }
    }
    out
}

/// The skills in one `skills/` folder: each direct subfolder holding a `SKILL.md`.
pub fn load_dir(root: &Path, source: &str, warnings: &mut Vec<String>) -> Vec<SkillDef> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut subdirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("SKILL.md").is_file()).collect();
    subdirs.sort();
    let mut out = vec![];
    for d in subdirs {
        match std::fs::read_to_string(d.join("SKILL.md"))
            .map_err(|e| e.to_string())
            .and_then(|t| parse_skill(&d, &t, source))
        {
            Ok(s) => out.push(s),
            Err(e) => warnings.push(format!("skill {}: {e}", d.display())),
        }
    }
    out
}

/// Skills from folders added with `--add-dir` (their `.claude/skills` and
/// `.forge/skills`), added to `skills`. One whose name is taken runs as
/// `<folder>:<name>`; between two added folders, the first keeps the plain name.
pub fn add_dir_skills(skills: &mut Vec<SkillDef>, dirs: &[PathBuf], warnings: &mut Vec<String>) {
    for dir in dirs {
        let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for root in [dir.join(".claude").join("skills"), dir.join(".forge").join("skills")] {
            for mut s in load_dir(&root, "add-dir", warnings) {
                if skills.iter().any(|o| o.name == s.name) {
                    s.name = format!("{folder}:{}", s.name);
                }
                if !skills.iter().any(|o| o.name == s.name) {
                    skills.push(s);
                }
            }
        }
    }
}

/// What the Skill tool reaches once the session exists: the runtime that
/// starts sub-agents (`context: fork`) and holds the engine and hooks, and
/// the session's files (for `paths`).
pub struct SkillLinks {
    pub rt: Arc<crate::AgentRuntime>,
    pub files: Arc<forge_tools::FileState>,
}

/// `Skill`: load a skill's instructions.
pub struct SkillTool {
    pub skills: Vec<SkillDef>,
    pub links: Arc<OnceLock<SkillLinks>>,
    /// Skills already loaded (name and arguments): loading one again adds a note, not a copy.
    loaded: Mutex<HashSet<String>>,
    /// Skills from `.claude/skills` in the project's subfolders, found once a
    /// file there is read or written, and the folders already looked in.
    nested: Mutex<Vec<SkillDef>>,
    scanned: Mutex<HashSet<PathBuf>>,
}

impl SkillTool {
    pub fn new(skills: Vec<SkillDef>) -> Self {
        Self::with_links(skills, Default::default())
    }

    pub fn with_links(skills: Vec<SkillDef>, links: Arc<OnceLock<SkillLinks>>) -> Self {
        SkillTool {
            skills,
            links,
            loaded: Mutex::new(HashSet::new()),
            nested: Mutex::new(vec![]),
            scanned: Mutex::new(HashSet::new()),
        }
    }

    /// The session's skills and those found in subfolders so far.
    fn all(&self) -> Vec<SkillDef> {
        let Some(l) = self.links.get() else { return self.skills.clone() };
        let project = l.rt.project_dir.clone();
        let mut nested = self.nested.lock().unwrap();
        let mut scanned = self.scanned.lock().unwrap();
        for file in l.files.touched() {
            let Some(parent) = file.parent() else { continue };
            for dir in parent.ancestors().take_while(|d| *d != project && d.starts_with(&project)) {
                if !scanned.insert(dir.to_path_buf()) {
                    continue;
                }
                let rel = dir.strip_prefix(&project).unwrap_or(dir).to_string_lossy().replace('\\', "/");
                for mut s in load_dir(&dir.join(".claude").join("skills"), "nested", &mut vec![]) {
                    if self.skills.iter().chain(nested.iter()).any(|o| o.name == s.name) {
                        s.name = format!("{rel}:{}", s.name);
                    }
                    nested.push(s);
                }
            }
        }
        self.skills.iter().chain(nested.iter()).cloned().collect()
    }

    /// The skills the model may load: model-invocable, and for one with
    /// `paths`, only once a matching file has been read or written.
    fn offered(&self) -> Vec<SkillDef> {
        let touched: Vec<PathBuf> = self.links.get().map(|l| l.files.touched()).unwrap_or_default();
        let project = self.links.get().map(|l| l.rt.project_dir.clone());
        self.all()
            .into_iter()
            .filter(|s| s.model_invocable && paths_match(&s.paths, &touched, project.as_deref()))
            .collect()
    }
}

/// Whether any of `touched` matches one of `paths` (globs relative to the project); no paths: yes.
fn paths_match(paths: &[String], touched: &[PathBuf], project: Option<&Path>) -> bool {
    if paths.is_empty() {
        return true;
    }
    let mut set = globset::GlobSetBuilder::new();
    for p in paths {
        if let Ok(g) = globset::GlobBuilder::new(p.trim_start_matches("./")).literal_separator(false).build() {
            set.add(g);
        }
    }
    let Ok(set) = set.build() else { return true };
    touched.iter().any(|t| {
        let rel = project.and_then(|p| t.strip_prefix(p).ok()).unwrap_or(t);
        set.is_match(rel)
    })
}

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> String {
        let mut list = String::new();
        for s in &self.offered() {
            let line =
                if s.name_only { format!("- {}\n", s.name) } else { format!("- {}: {}\n", s.name, s.description) };
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
        self.skills.iter().any(|s| s.model_invocable)
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        let name = input.get("skill").and_then(Value::as_str).unwrap_or_default().trim_start_matches('/');
        let args = input.get("args").and_then(Value::as_str).unwrap_or_default().trim();
        // `Skill(name)` matches the bare call, `Skill(name *)` any arguments.
        Subject::Name(if args.is_empty() { name.to_string() } else { format!("{name} {args}") })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let name = input.get("skill").and_then(Value::as_str).unwrap_or_default().trim_start_matches('/');
        let args = input.get("args").and_then(Value::as_str).unwrap_or_default();
        // A skill limited by `paths` may still be named outright.
        let all = self.all();
        let Some(s) =
            all.iter().filter(|s| s.model_invocable).find(|s| s.name == name || s.name.eq_ignore_ascii_case(name))
        else {
            let names: Vec<String> = self.offered().into_iter().map(|s| s.name).collect();
            return ToolOutput::error(format!("Unknown skill {name:?}. Available: {}", names.join(", ")));
        };
        let links = self.links.get();
        let handle = links.and_then(|l| l.rt.parent.get()).map(|p| &p.handle);
        let session = crate::invoke::Session { handle, hooks: links.map(|l| &l.rt.hooks) };
        let structured = json!({"skill": s.name, "success": true});
        match crate::invoke::invoke(s, args, ctx, session).await {
            Err(e) => ToolOutput::error(e),
            Ok(crate::invoke::Invoked::Fork { agent, prompt }) => {
                let Some(l) = links else {
                    return ToolOutput::error("This skill runs in a sub-agent, which this session cannot start.");
                };
                let task = crate::TaskTool { rt: l.rt.clone() };
                let input = json!({"description": format!("/{}", s.name), "prompt": prompt, "subagent_type": agent});
                if let Err(e) = task.validate(&input, ctx) {
                    return ToolOutput::error(format!("/{}: {e}", s.name));
                }
                task.call(input, ctx).await
            }
            Ok(crate::invoke::Invoked::Text(text)) => {
                let first = self.loaded.lock().unwrap().insert(format!("{}\u{0}{}", s.name, args.trim()));
                if first {
                    ToolOutput::text(text).with_structured(structured)
                } else {
                    ToolOutput::text(format!(
                        "The {} skill is already loaded in this conversation (above); follow it.",
                        s.name
                    ))
                    .with_structured(structured)
                }
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
        let tool = SkillTool::new(skills);
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
        let mut skills = load_skills(d.path(), &plugins, &mut w);
        let m = skills.iter_mut().find(|s| s.name == "migrate").unwrap();
        assert_eq!(m.allowed_tools, ["Bash(${CLAUDE_SKILL_DIR}/run.sh *)", "Read"]);
        assert!(m.fork && m.agent.as_deref() == Some("Explore") && m.model.is_none());
        // (Run inline here, to check its text; a fork goes to a sub-agent.)
        m.fork = false;
        let tool = SkillTool::new(skills.clone());
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
        let ctx = ToolContext::new(Path::new("/p"));
        let Ok(crate::invoke::Invoked::Text(t)) = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(crate::invoke::invoke(review, "--fix 12", &ctx, Default::default()))
        else {
            panic!()
        };
        assert!(t.starts_with("Review code changes"));
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
