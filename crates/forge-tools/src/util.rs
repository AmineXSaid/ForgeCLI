use std::path::{Path, PathBuf};

/// Resolve a tool's path argument against the project directory.
pub fn expand_path(raw: &str, base: &Path) -> PathBuf {
    forge_permissions::normalize(Path::new(raw.trim()), base)
}

/// Keep the head and tail of long output, eliding the middle.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let mut head_end = half;
    while !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len() - half;
    while !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let dropped = s[head_end..tail_start].lines().count();
    format!("{}\n\n... [{dropped} lines truncated] ...\n\n{}", &s[..head_end], &s[tail_start..])
}

/// `cat -n` style numbering used by Read and in edit snippets.
pub fn number_lines(lines: &[&str], first: usize) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        out.push_str(&format!("{:>6}\t{}\n", first + i, l));
    }
    out
}
