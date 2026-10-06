use forge_permissions::Subject;
use serde_json::{json, Value};
use similar::TextDiff;

use super::str_arg;
use crate::util::number_lines;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

/// Replace `old` with `new` in `content`, enforcing uniqueness unless `all`.
pub fn apply_edit(content: &str, old: &str, new: &str, all: bool) -> Result<String, String> {
    if old == new {
        return Err("No changes to make: old_string and new_string are exactly the same.".into());
    }
    if old.is_empty() {
        if content.is_empty() {
            return Ok(new.to_string());
        }
        return Err("old_string is empty but the file is not; use Write to replace a whole file.".into());
    }
    let count = content.matches(old).count();
    match count {
        0 => Err(format!("String to replace not found in file.\nString: {old}")),
        1 => Ok(content.replacen(old, new, 1)),
        n if all => {
            let _ = n;
            Ok(content.replace(old, new))
        }
        n => Err(format!(
            "Found {n} matches of the string to replace, but replace_all is false. To replace all occurrences, \
             set replace_all to true. To replace only one occurrence, provide more context to uniquely identify \
             the instance.\nString: {old}"
        )),
    }
}

/// Unified-diff hunks for SDK hosts (`structuredPatch`).
pub fn patch_hunks(old: &str, new: &str) -> Vec<Value> {
    let diff = TextDiff::from_lines(old, new);
    diff.unified_diff()
        .context_radius(3)
        .iter_hunks()
        .map(|h| {
            let ops = h.ops();
            let (first, last) = (ops.first(), ops.last());
            let old_start = first.map(|o| o.old_range().start + 1).unwrap_or(1);
            let new_start = first.map(|o| o.new_range().start + 1).unwrap_or(1);
            let old_lines = last.map(|o| o.old_range().end + 1).unwrap_or(1) - old_start;
            let new_lines = last.map(|o| o.new_range().end + 1).unwrap_or(1) - new_start;
            let lines: Vec<String> = h
                .iter_changes()
                .map(|c| {
                    let sign = match c.tag() {
                        similar::ChangeTag::Delete => '-',
                        similar::ChangeTag::Insert => '+',
                        similar::ChangeTag::Equal => ' ',
                    };
                    format!("{sign}{}", c.value().trim_end_matches('\n'))
                })
                .collect();
            json!({"oldStart": old_start, "oldLines": old_lines, "newStart": new_start, "newLines": new_lines, "lines": lines})
        })
        .collect()
}

/// A few numbered lines around the first change, shown to the model.
fn snippet(new_content: &str, new_string: &str) -> String {
    let lines: Vec<&str> = new_content.lines().collect();
    let at = new_content.find(new_string).map(|pos| new_content[..pos].lines().count().max(1)).unwrap_or(1);
    let start = at.saturating_sub(4).max(1);
    let end = (at + new_string.lines().count() + 4).min(lines.len());
    if start > end || lines.is_empty() {
        return String::new();
    }
    number_lines(&lines[start - 1..end], start)
}

fn edit_subject(input: &Value, ctx: &ToolContext) -> Subject {
    Subject::Path { path: expand_path(str_arg(input, "file_path"), &ctx.project_dir), write: true }
}

fn write_result(ctx: &ToolContext, path: &std::path::Path, old: &str, new: &str) -> Result<Vec<Value>, String> {
    ctx.checkpoint(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, new).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    ctx.files.record_write(path);
    Ok(patch_hunks(old, new))
}

pub struct Edit;

#[async_trait::async_trait]
impl Tool for Edit {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> String {
        "Replace an exact string in a file.\n\n\
         - Read the file first; edits to unread or since-modified files are refused.\n\
         - `old_string` must match the file exactly (indentation included) and be unique unless \
           `replace_all` is true; add surrounding lines to make it unique.\n\
         - Copy text from Read output without the line-number prefix.\n\
         - An empty `old_string` on a missing or empty file creates it with `new_string`."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "The absolute path to the file to modify"},
                "old_string": {"type": "string", "description": "The text to replace"},
                "new_string": {"type": "string", "description": "The text to replace it with (must differ from old_string)"},
                "replace_all": {"type": "boolean", "default": false, "description": "Replace every occurrence of old_string"}
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false
        })
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        edit_subject(input, ctx)
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        let path = expand_path(str_arg(input, "file_path"), &ctx.project_dir);
        if !path.exists() && !str_arg(input, "old_string").is_empty() {
            return Err(format!("File does not exist: {}", path.display()));
        }
        ctx.files.check_writable(&path)
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let path = expand_path(str_arg(&input, "file_path"), &ctx.project_dir);
        let (old_s, new_s) = (str_arg(&input, "old_string"), str_arg(&input, "new_string"));
        let all = input.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        // Files may use CRLF; match against normalised text and keep the style.
        let crlf = content.contains("\r\n");
        let norm = if crlf { content.replace("\r\n", "\n") } else { content.clone() };
        let updated = match apply_edit(&norm, &old_s.replace("\r\n", "\n"), &new_s.replace("\r\n", "\n"), all) {
            Ok(u) => u,
            Err(e) => return ToolOutput::error(e),
        };
        let out = if crlf { updated.replace('\n', "\r\n") } else { updated.clone() };
        match write_result(ctx, &path, &content, &out) {
            Ok(patch) => ToolOutput::text(format!(
                "The file {} has been updated. Here's the result of running `cat -n` on a snippet of the edited file:\n{}",
                path.display(),
                snippet(&updated, new_s)
            ))
            .with_structured(json!({
                "filePath": path, "oldString": old_s, "newString": new_s, "replaceAll": all,
                "originalFile": content, "structuredPatch": patch,
            })),
            Err(e) => ToolOutput::error(e),
        }
    }
}

pub struct MultiEdit;

#[async_trait::async_trait]
impl Tool for MultiEdit {
    fn name(&self) -> &str {
        "MultiEdit"
    }

    fn description(&self) -> String {
        "Make several exact-string replacements in one file, applied in order and atomically (all or none). \
         Each edit follows Edit's rules and sees the result of the previous one."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "The absolute path to the file to modify"},
                "edits": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {"type": "string"},
                            "new_string": {"type": "string"},
                            "replace_all": {"type": "boolean", "default": false}
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            },
            "required": ["file_path", "edits"],
            "additionalProperties": false
        })
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        edit_subject(input, ctx)
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        let path = expand_path(str_arg(input, "file_path"), &ctx.project_dir);
        ctx.files.check_writable(&path)
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let path = expand_path(str_arg(&input, "file_path"), &ctx.project_dir);
        let original = std::fs::read_to_string(&path).unwrap_or_default();
        let mut content = original.clone();
        let edits = input.get("edits").and_then(Value::as_array).cloned().unwrap_or_default();
        if edits.is_empty() {
            return ToolOutput::error("edits must contain at least one edit");
        }
        for (i, e) in edits.iter().enumerate() {
            let all = e.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
            match apply_edit(&content, str_arg(e, "old_string"), str_arg(e, "new_string"), all) {
                Ok(c) => content = c,
                Err(err) => {
                    return ToolOutput::error(format!(
                        "Edit {} of {} failed; no changes were made. {err}",
                        i + 1,
                        edits.len()
                    ))
                }
            }
        }
        match write_result(ctx, &path, &original, &content) {
            Ok(patch) => ToolOutput::text(format!("Applied {} edits to {}", edits.len(), path.display()))
                .with_structured(
                    json!({"filePath": path, "edits": edits, "originalFile": original, "structuredPatch": patch}),
                ),
            Err(e) => ToolOutput::error(e),
        }
    }
}
