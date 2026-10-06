//! Output styles: how the agent writes, not what it can do. `default` adds
//! nothing; `explanatory` and `learning` are Forge's own; custom styles are
//! Markdown files in `output-styles/`. Chosen with the `outputStyle` setting.

use std::path::Path;

use crate::frontmatter;

#[derive(Debug, Clone, PartialEq)]
pub struct OutputStyle {
    pub name: String,
    pub description: String,
    /// Text added to the system prompt (empty for `default`).
    pub prompt: String,
    pub source: String,
}

const EXPLANATORY: &str = "While you work, help the user understand the codebase and your choices. \
Before and after significant steps, add a short \"Insight\" note (two or three sentences) about why the code \
is structured as it is, the trade-off you chose, or a pattern worth knowing. Keep insights specific to this \
code, not general programming advice, and keep doing the task itself as well as you otherwise would.";

const LEARNING: &str = "The user wants to learn by doing. Do the scaffolding and the parts that need wide \
context yourself, but for small, meaningful pieces of logic (about 2-10 lines: a decision, an algorithm \
step, an error-handling choice), leave a clearly marked `TODO(human)` in the code and ask the user to write it, \
explaining what it should do and what to consider. Ask for at most one such piece at a time, then wait. Add \
short insights about the code's design as you go.";

pub fn builtin_styles() -> Vec<OutputStyle> {
    vec![
        OutputStyle {
            name: "default".into(),
            description: "Concise and task-focused".into(),
            prompt: String::new(),
            source: "built-in".into(),
        },
        OutputStyle {
            name: "explanatory".into(),
            description: "Explains design choices and patterns as it works".into(),
            prompt: EXPLANATORY.into(),
            source: "built-in".into(),
        },
        OutputStyle {
            name: "learning".into(),
            description: "Hands small pieces of the work to you, to learn by doing".into(),
            prompt: LEARNING.into(),
            source: "built-in".into(),
        },
    ]
}

/// Built-ins, then user, project and plugin `output-styles/*.md` (later wins by name).
pub fn load_styles(project: &Path, plugins: &[crate::plugins::Plugin], warnings: &mut Vec<String>) -> Vec<OutputStyle> {
    let mut out = builtin_styles();
    let mut dirs: Vec<(std::path::PathBuf, String)> = forge_config::resource_dirs("output-styles", project)
        .into_iter()
        .map(|(scope, d)| (d, if scope == forge_config::Scope::User { "user" } else { "project" }.to_string()))
        .collect();
    dirs.extend(plugins.iter().map(|p| (p.dir.join("output-styles"), p.name.clone())));
    for (dir, source) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> =
            rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                warnings.push(format!("output style {}: unreadable", f.display()));
                continue;
            };
            let (fm, body) = frontmatter::parse(&text);
            let name = fm
                .text("name")
                .map(str::to_string)
                .unwrap_or_else(|| f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            out.retain(|s| !s.name.eq_ignore_ascii_case(&name));
            out.push(OutputStyle {
                description: fm.text("description").unwrap_or_default().to_string(),
                name,
                prompt: body.trim().to_string(),
                source: source.clone(),
            });
        }
    }
    out
}

pub fn find<'a>(styles: &'a [OutputStyle], name: &str) -> Option<&'a OutputStyle> {
    styles.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_and_custom_styles() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".forge/output-styles")).unwrap();
        std::fs::write(
            d.path().join(".forge/output-styles/terse.md"),
            "---\ndescription: Very short\n---\nAnswer in one line.",
        )
        .unwrap();
        let styles = load_styles(d.path(), &[], &mut vec![]);
        assert_eq!(
            styles.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["default", "explanatory", "learning", "terse"]
        );
        assert_eq!(find(&styles, "Terse").unwrap().prompt, "Answer in one line.");
        assert!(find(&styles, "default").unwrap().prompt.is_empty());
    }
}
