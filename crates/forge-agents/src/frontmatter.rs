//! `---` frontmatter for Markdown resources (commands, skills, output styles):
//! a small YAML subset of `key: value`, inline `[a, b]` lists, `- item` lists
//! and `>` / `|` folded or literal blocks.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Text(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub fields: BTreeMap<String, Field>,
    /// Single-line values as written (`argument-hint: [file]` is text, not a list).
    raw: BTreeMap<String, String>,
}

impl Frontmatter {
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.fields.get(key) {
            Some(Field::Text(t)) => Some(t.as_str()),
            _ => self.raw.get(key).map(String::as_str),
        }
    }

    /// A list field, or a comma/space separated text field read as a list.
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        match self.fields.get(key)? {
            Field::List(l) => Some(l.clone()),
            Field::Text(t) => Some(
                t.split([',', ' '])
                    .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            ),
        }
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.text(key)?.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// Split `text` into frontmatter and body. Text without frontmatter has empty fields.
pub fn parse(text: &str) -> (Frontmatter, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text.strip_prefix("---").filter(|r| r.starts_with('\n') || r.starts_with("\r\n")) else {
        return (Frontmatter::default(), text.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (Frontmatter::default(), text.to_string());
    };
    let front = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['-']).trim_start_matches(['\r', '\n']).to_string();
    let mut fm = Frontmatter::default();
    let lines: Vec<&str> = front.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line.starts_with(' ') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (key, v) = (k.trim().to_string(), v.trim());
        if v == ">" || v == "|" || v == ">-" || v == "|-" {
            let mut block = vec![];
            while i < lines.len() && (lines[i].starts_with(' ') || lines[i].trim().is_empty()) {
                block.push(lines[i].trim());
                i += 1;
            }
            let joined = if v.starts_with('>') { block.join(" ") } else { block.join("\n") };
            fm.fields.insert(key, Field::Text(joined.trim().to_string()));
        } else if v.is_empty() {
            let mut items = vec![];
            while i < lines.len() && lines[i].trim_start().starts_with("- ") {
                items.push(unquote(&lines[i].trim_start()[2..]));
                i += 1;
            }
            fm.fields.insert(key, if items.is_empty() { Field::Text(String::new()) } else { Field::List(items) });
        } else if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let items = inner.split(',').map(unquote).filter(|s| !s.is_empty()).collect();
            fm.raw.insert(key.clone(), v.to_string());
            fm.fields.insert(key, Field::List(items));
        } else {
            fm.fields.insert(key, Field::Text(unquote(v)));
        }
    }
    (fm, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_yaml_subset() {
        let text = "---\nname: review\ndescription: \"Review: the diff\"\nallowed-tools: [Read, \"Bash(git diff:*)\"]\ntags:\n  - a\n  - 'b'\nlong: >\n  folded\n  text\nflag: yes\n---\nBody line\n";
        let (fm, body) = parse(text);
        assert_eq!(fm.text("name"), Some("review"));
        assert_eq!(fm.text("description"), Some("Review: the diff"));
        assert_eq!(fm.list("allowed-tools").unwrap(), ["Read", "Bash(git diff:*)"]);
        assert_eq!(fm.list("tags").unwrap(), ["a", "b"]);
        assert_eq!(fm.text("long"), Some("folded text"));
        assert_eq!(fm.flag("flag"), Some(true));
        assert_eq!(body, "Body line\n");
        let (fm, body) = parse("no frontmatter\n---\nhere");
        assert!(fm.fields.is_empty());
        assert_eq!(body, "no frontmatter\n---\nhere");
        assert_eq!(parse("---\nmodel: haiku\n---\n").0.list("model").unwrap(), ["haiku"]);
    }
}
