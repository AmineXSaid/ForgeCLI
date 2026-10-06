//! HTML to Markdown, enough for reading documentation and articles.
//!
//! It drops scripts, styles and navigation chrome. It keeps headings, links,
//! lists, code, emphasis and tables (as rows), and decodes entities.

use std::sync::LazyLock;

use regex::Regex;

static DROP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<!--.*?-->|<(script|style|noscript|svg|template|iframe|canvas)\b[^>]*>.*?</(script|style|noscript|svg|template|iframe|canvas)\s*>")
        .expect("regex")
});
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("regex"));
static BODY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<body[^>]*>(.*)</body>").expect("regex"));
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<(/?)([a-zA-Z][a-zA-Z0-9]*)([^>]*)>").expect("regex"));
static HREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).expect("regex"));
static ALT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\balt\s*=\s*(?:"([^"]*)"|'([^']*)')"#).expect("regex"));
static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").expect("regex"));

pub fn decode_entities(s: &str) -> String {
    ENTITY
        .replace_all(s, |c: &regex::Captures| {
            let e = &c[1];
            let ch = if let Some(hex) = e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(dec) = e.strip_prefix('#') {
                dec.parse().ok().and_then(char::from_u32)
            } else {
                match e {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "hellip" => Some('…'),
                    "copy" => Some('©'),
                    "rsquo" | "lsquo" => Some('\''),
                    "rdquo" | "ldquo" => Some('"'),
                    _ => None,
                }
            };
            ch.map(|c| c.to_string()).unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

fn attr(re: &Regex, attrs: &str) -> Option<String> {
    re.captures(attrs)
        .and_then(|c| c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)).map(|m| decode_entities(m.as_str())))
}

/// Convert an HTML document to Markdown. `base` resolves relative links.
pub fn to_markdown(html: &str, base: Option<&str>) -> String {
    let title = TITLE.captures(html).map(|c| decode_entities(c[1].trim()));
    let cleaned = DROP.replace_all(html, "");
    let body = BODY.captures(&cleaned).map(|c| c[1].to_string()).unwrap_or_else(|| cleaned.to_string());

    let mut out = String::new();
    let mut pre = 0usize;
    let mut links: Vec<(String, usize)> = vec![]; // href, start offset in out
    let mut lists: Vec<Option<usize>> = vec![]; // None = ul, Some(n) = ol counter
    let mut skip_depth = 0usize; // inside <nav>, <header>, <footer>, <aside>, <form>, <button>
    let mut last = 0;
    let resolve = |href: &str| -> String {
        match (base, href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:")) {
            (Some(b), false) => {
                reqwest::Url::parse(b).and_then(|u| u.join(href)).map(|u| u.to_string()).unwrap_or(href.to_string())
            }
            _ => href.to_string(),
        }
    };
    let text = |s: &str, pre: usize| -> String {
        let t = decode_entities(s);
        if pre > 0 {
            t
        } else {
            let collapsed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
            let lead = if t.starts_with(char::is_whitespace) && !collapsed.is_empty() { " " } else { "" };
            let trail = if t.ends_with(char::is_whitespace) && !collapsed.is_empty() { " " } else { "" };
            format!("{lead}{collapsed}{trail}")
        }
    };
    for m in TAG.captures_iter(&body) {
        let whole = m.get(0).unwrap();
        if skip_depth == 0 {
            out.push_str(&text(&body[last..whole.start()], pre));
        }
        last = whole.end();
        let closing = &m[1] == "/";
        let name = m[2].to_ascii_lowercase();
        let attrs = &m[3];
        if matches!(name.as_str(), "nav" | "footer" | "aside" | "form" | "button" | "select") {
            if closing {
                skip_depth = skip_depth.saturating_sub(1);
            } else if !attrs.trim_end().ends_with('/') {
                skip_depth += 1;
            }
            continue;
        }
        if skip_depth > 0 {
            continue;
        }
        match (name.as_str(), closing) {
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                let level = name[1..].parse::<usize>().unwrap_or(1);
                out.push_str(&format!("\n\n{} ", "#".repeat(level)));
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => out.push_str("\n\n"),
            ("p" | "div" | "section" | "article" | "main" | "header" | "table" | "blockquote" | "figure" | "dl", _) => {
                out.push_str("\n\n")
            }
            ("br", _) => out.push('\n'),
            ("hr", _) => out.push_str("\n\n---\n\n"),
            ("tr", false) | ("dt", false) | ("dd", false) => out.push('\n'),
            ("td" | "th", false) => out.push_str(" | "),
            ("ul", false) => lists.push(None),
            ("ol", false) => lists.push(Some(0)),
            ("ul" | "ol", true) => {
                lists.pop();
                out.push('\n');
            }
            ("li", false) => {
                let indent = "  ".repeat(lists.len().saturating_sub(1));
                let bullet = match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{n}.")
                    }
                    _ => "-".into(),
                };
                out.push_str(&format!("\n{indent}{bullet} "));
            }
            ("pre", false) => {
                pre += 1;
                out.push_str("\n\n```\n");
            }
            ("pre", true) => {
                pre = pre.saturating_sub(1);
                out.push_str("\n```\n\n");
            }
            ("code", _) if pre == 0 => out.push('`'),
            ("strong" | "b", _) => out.push_str("**"),
            ("em" | "i", _) => out.push('*'),
            ("a", false) => {
                if let Some(h) = attr(&HREF, attrs) {
                    links.push((resolve(&h), out.len()));
                    out.push('[');
                }
            }
            ("a", true) => {
                if let Some((href, start)) = links.pop() {
                    let label = out[start + 1..].trim().to_string();
                    if label.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
                        out.truncate(start);
                        out.push_str(&label);
                    } else {
                        out.truncate(start);
                        out.push_str(&format!("[{label}]({href})"));
                    }
                }
            }
            ("img", _) => {
                if let Some(alt) = attr(&ALT, attrs).filter(|a| !a.trim().is_empty()) {
                    out.push_str(&format!("[image: {alt}]"));
                }
            }
            _ => {}
        }
    }
    if skip_depth == 0 {
        out.push_str(&text(&body[last..], pre));
    }
    // Tidy: trim each line, at most one blank line in a row.
    let mut tidy = String::new();
    let mut blank = 0;
    let mut in_fence = false;
    for line in out.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        let l = if in_fence { line.trim_end() } else { line.trim() };
        if l.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        tidy.push_str(l);
        tidy.push('\n');
    }
    let body = tidy.trim().to_string();
    match title.filter(|t| !t.is_empty()) {
        Some(t) if !body.starts_with(&format!("# {t}")) => format!("# {t}\n\n{body}"),
        _ => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_typical_page() {
        let html = r#"<!doctype html><html><head><title>Guide &amp; Notes</title>
            <style>body { color: red }</style><script>alert("x")</script></head>
            <body><nav><a href="/">Home</a> <a href="/docs">Docs</a></nav>
            <h2>Install</h2><p>Run   the <code>install</code> command, then <a href="/start">read on</a>.</p>
            <pre><code>cargo install forge
forge --help</code></pre>
            <ul><li>One</li><li>Two <b>bold</b></li></ul><ol><li>First</li><li>Second</li></ol>
            <p>5 &lt; 6 &#x26; caf&eacute;&nbsp;ok</p><img src="a.png" alt="diagram">
            <footer>© corp</footer></body></html>"#;
        let md = to_markdown(html, Some("https://example.com/guide/"));
        assert!(md.starts_with("# Guide & Notes\n\n## Install"), "{md}");
        assert!(md.contains("Run the `install` command, then [read on](https://example.com/start)."), "{md}");
        assert!(md.contains("```\ncargo install forge\nforge --help\n```"), "{md}");
        assert!(md.contains("- One\n- Two **bold**"), "{md}");
        assert!(md.contains("1. First\n2. Second"), "{md}");
        assert!(md.contains("5 < 6 & caf&eacute; ok"), "unknown entities stay: {md}");
        assert!(md.contains("[image: diagram]"));
        assert!(!md.contains("alert") && !md.contains("color: red") && !md.contains("Home") && !md.contains("corp"));
    }
}
