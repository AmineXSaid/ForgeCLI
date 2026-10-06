/// Split a shell command line into its simple commands at `&&`, `||`, `;`,
/// `|`, `&` and newlines, ignoring separators inside quotes. Subshell and
/// substitution syntax (`$(...)`, backticks) is kept inside its command so a
/// rule never matches only the outer part.
pub fn split_compound(cmd: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let (mut single, mut double, mut paren, mut tick) = (false, false, 0i32, false);
    while let Some(c) = chars.next() {
        match c {
            '\\' if !single => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                continue;
            }
            '\'' if !double && !tick => single = !single,
            '"' if !single && !tick => double = !double,
            '`' if !single => tick = !tick,
            '(' if !single && !double => paren += 1,
            ')' if !single && !double => paren -= 1,
            _ => {}
        }
        let quoted = single || double || tick || paren > 0;
        if !quoted && matches!(c, ';' | '\n' | '|' | '&') {
            // `>&` / `2>&1` and `&>` are redirections, not separators.
            if c == '&' && (cur.ends_with('>') || chars.peek() == Some(&'>')) {
                cur.push(c);
                continue;
            }
            if (c == '|' && chars.peek() == Some(&'|')) || (c == '&' && chars.peek() == Some(&'&')) {
                chars.next();
            }
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
            continue;
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}
