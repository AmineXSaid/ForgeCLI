//! A PDF's text, for endpoints that can't take the PDF itself.
//!
//! The Messages API reads an attached PDF natively (a `document` block). An
//! OpenAI-compatible endpoint has no such block: the translation used to drop
//! it, or flatten it into its base64. This gives its text instead, page by
//! page: lopdf reads the document (object streams, fonts' ToUnicode maps,
//! which is how most exported PDFs encode their text); when lopdf can't load
//! it, the content streams are scanned directly (the `Tj`, `TJ`, `'` and `"`
//! operators inside `BT … ET`, Flate streams inflated first). A scanned PDF,
//! or one whose fonts have no Unicode map, yields little or noise:
//! [`extract`] returns None then, and the caller says so instead of passing
//! noise on.

use std::io::Read;

/// The most text taken from one PDF.
pub const MAX_CHARS: usize = 200_000;

/// The most pages read.
const MAX_PAGES: usize = 300;

/// The text of `pdf`, or None when it draws no readable text.
pub fn extract(pdf: &[u8]) -> Option<String> {
    with_lopdf(pdf).or_else(|| scan(pdf))
}

fn with_lopdf(pdf: &[u8]) -> Option<String> {
    let doc = lopdf::Document::load_mem(pdf).ok()?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().take(MAX_PAGES).collect();
    let many = pages.len() > 1;
    let mut out = String::new();
    for n in pages {
        let text = doc.extract_text(&[n]).unwrap_or_default();
        if many {
            out.push_str(&format!("--- Page {n} ---\n"));
        }
        out.push_str(text.trim());
        out.push_str("\n\n");
        if out.len() > MAX_CHARS {
            break;
        }
    }
    let text = tidy(&out);
    readable(&text).then_some(text)
}

fn scan(pdf: &[u8]) -> Option<String> {
    let mut out = String::new();
    for stream in content_streams(pdf) {
        text_of(&stream, &mut out);
        if out.len() > MAX_CHARS {
            break;
        }
    }
    let text = tidy(&out);
    readable(&text).then_some(text)
}

/// Every stream's bytes, inflated when it is Flate-encoded; images, fonts
/// and other binary streams are left out.
fn content_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(start) = find(pdf, b"stream", at) {
        // `endstream` also contains `stream`.
        if start >= 3 && &pdf[start - 3..start] == b"end" {
            at = start + 6;
            continue;
        }
        let mut body = start + 6;
        if pdf.get(body) == Some(&b'\r') {
            body += 1;
        }
        if pdf.get(body) == Some(&b'\n') {
            body += 1;
        }
        let Some(end) = find(pdf, b"endstream", body) else { break };
        let dict_from = rfind(&pdf[..start], b"<<").unwrap_or(start.saturating_sub(512));
        let dict = &pdf[dict_from..start];
        at = end + 9;
        if contains(dict, b"/Image")
            || contains(dict, b"/FontFile")
            || contains(dict, b"/Length1")
            || contains(dict, b"/XRef")
        {
            continue;
        }
        let raw = &pdf[body..end];
        let data = if contains(dict, b"/FlateDecode") {
            let mut d = Vec::new();
            match flate2::read::ZlibDecoder::new(raw).take(16 * 1024 * 1024).read_to_end(&mut d) {
                Ok(_) => d,
                Err(_) if !d.is_empty() => d,
                Err(_) => continue,
            }
        } else if contains(dict, b"/Filter") {
            // Another filter (DCT, LZW…): not text this reads.
            continue;
        } else {
            raw.to_vec()
        };
        out.push(data);
    }
    out
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle, 0).is_some()
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(Vec<u8>),
    Num(f64),
    ArrOpen,
    ArrClose,
    Op(String),
}

/// The tokens of a content stream that matter for text.
fn tokens(s: &[u8]) -> Vec<Tok> {
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            b'(' => {
                let mut depth = 1;
                let mut buf = vec![];
                i += 1;
                while i < s.len() && depth > 0 {
                    let c = s[i];
                    match c {
                        b'\\' if i + 1 < s.len() => {
                            i += 1;
                            let e = s[i];
                            match e {
                                b'n' => buf.push(b'\n'),
                                b'r' => buf.push(b'\r'),
                                b't' => buf.push(b'\t'),
                                b'b' | b'f' => {}
                                b'0'..=b'7' => {
                                    let mut v = 0u32;
                                    let mut k = 0;
                                    while k < 3 && i < s.len() && (b'0'..=b'7').contains(&s[i]) {
                                        v = v * 8 + (s[i] - b'0') as u32;
                                        i += 1;
                                        k += 1;
                                    }
                                    i -= 1;
                                    buf.push(v as u8);
                                }
                                b'\r' | b'\n' => {}
                                other => buf.push(other),
                            }
                        }
                        b'(' => {
                            depth += 1;
                            buf.push(c);
                        }
                        b')' => {
                            depth -= 1;
                            if depth > 0 {
                                buf.push(c);
                            }
                        }
                        _ => buf.push(c),
                    }
                    i += 1;
                }
                out.push(Tok::Str(buf));
                continue;
            }
            b'<' if s.get(i + 1) == Some(&b'<') => i += 2,
            b'>' if s.get(i + 1) == Some(&b'>') => i += 2,
            b'<' => {
                let end = s[i..].iter().position(|&b| b == b'>').map_or(s.len(), |p| i + p);
                let hex: Vec<u8> = s[i + 1..end].iter().copied().filter(u8::is_ascii_hexdigit).collect();
                let bytes = hex
                    .chunks(2)
                    .map(|p| {
                        let h = std::str::from_utf8(p).unwrap_or("0");
                        u8::from_str_radix(&format!("{h:0<2}"), 16).unwrap_or(0)
                    })
                    .collect();
                out.push(Tok::Str(bytes));
                i = end + 1;
            }
            b'[' => {
                out.push(Tok::ArrOpen);
                i += 1;
            }
            b']' => {
                out.push(Tok::ArrClose);
                i += 1;
            }
            b'%' => {
                while i < s.len() && s[i] != b'\n' && s[i] != b'\r' {
                    i += 1;
                }
            }
            b'/' => {
                i += 1;
                while i < s.len() && !b" \t\r\n/[]()<>{}%".contains(&s[i]) {
                    i += 1;
                }
            }
            c if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' => {
                let start = i;
                i += 1;
                while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'.') {
                    i += 1;
                }
                let n = std::str::from_utf8(&s[start..i]).ok().and_then(|t| t.parse().ok()).unwrap_or(0.0);
                out.push(Tok::Num(n));
            }
            c if c.is_ascii_alphabetic() || c == b'\'' || c == b'"' || c == b'*' => {
                let start = i;
                i += 1;
                while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'*' || s[i] == b'\'' || s[i] == b'"') {
                    i += 1;
                }
                out.push(Tok::Op(String::from_utf8_lossy(&s[start..i]).into_owned()));
            }
            _ => i += 1,
        }
    }
    out
}

/// A PDF string as text: UTF-16 when it says so, else Latin-1 (close to
/// PDFDocEncoding and WinAnsi for the characters that matter).
fn decode(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = b[2..].chunks(2).map(|p| u16::from_be_bytes([p[0], *p.get(1).unwrap_or(&0)])).collect();
        return String::from_utf16_lossy(&units);
    }
    b.iter()
        .map(|&c| match c {
            0x91 | 0x92 => '\'',
            0x93 | 0x94 => '"',
            0x95 => '•',
            0x96 | 0x97 => '-',
            c => c as char,
        })
        .collect()
}

/// Append the text one content stream draws.
fn text_of(stream: &[u8], out: &mut String) {
    let toks = tokens(stream);
    let mut in_text = false;
    let mut nums: Vec<f64> = vec![];
    let mut array: Option<Vec<Tok>> = None;
    let mut last_str: Option<Vec<u8>> = None;
    for t in toks {
        if let Some(a) = array.as_mut() {
            if t == Tok::ArrClose {
                let items = array.take().unwrap_or_default();
                // The TJ array: kerning numbers, a wide gap meaning a space.
                let mut s = String::new();
                for it in items {
                    match it {
                        Tok::Str(b) => s.push_str(&decode(&b)),
                        Tok::Num(n) if n < -200.0 => s.push(' '),
                        _ => {}
                    }
                }
                last_str = Some(s.into_bytes());
            } else {
                a.push(t);
            }
            continue;
        }
        match t {
            Tok::ArrOpen => array = Some(vec![]),
            Tok::Str(b) => last_str = Some(decode(&b).into_bytes()),
            Tok::Num(n) => nums.push(n),
            Tok::Op(op) => {
                match op.as_str() {
                    "BT" => in_text = true,
                    "ET" => {
                        in_text = false;
                        out.push('\n');
                    }
                    "Tj" | "TJ" if in_text => {
                        if let Some(s) = last_str.take() {
                            out.push_str(&String::from_utf8_lossy(&s));
                        }
                    }
                    "'" | "\"" if in_text => {
                        out.push('\n');
                        if let Some(s) = last_str.take() {
                            out.push_str(&String::from_utf8_lossy(&s));
                        }
                    }
                    "T*" if in_text => out.push('\n'),
                    "Td" | "TD" if in_text => {
                        // A move down starts a new line; a move along, a space.
                        let dy = nums.last().copied().unwrap_or(0.0);
                        if dy.abs() > 0.1 {
                            out.push('\n');
                        } else if !out.ends_with([' ', '\n']) {
                            out.push(' ');
                        }
                    }
                    "Tm" if in_text && !out.ends_with('\n') => out.push('\n'),
                    _ => {}
                }
                nums.clear();
                last_str = None;
            }
            Tok::ArrClose => {}
        }
    }
}

/// Collapse runs of blank lines and trailing spaces.
fn tidy(s: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
        if out.len() > MAX_CHARS {
            break;
        }
    }
    out.trim().to_string()
}

/// Enough letters, and few control characters: text, not encoded glyph ids.
fn readable(s: &str) -> bool {
    let total = s.chars().filter(|c| !c.is_whitespace()).count();
    if total < 4 {
        return false;
    }
    let good = s.chars().filter(|c| c.is_alphanumeric() || c.is_ascii_punctuation() || "•€£–—’“”".contains(*c)).count();
    let control = s.chars().filter(|c| c.is_control() && !c.is_whitespace()).count();
    good * 100 >= total * 80 && control * 100 <= total * 5
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF, its content stream compressed or not.
    pub fn pdf(content: &[u8], compress: bool) -> Vec<u8> {
        let (body, filter) = if compress {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut e, content).unwrap();
            (e.finish().unwrap(), " /Filter /FlateDecode")
        } else {
            (content.to_vec(), "")
        };
        let mut out = b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n".to_vec();
        out.extend(format!("4 0 obj << /Length {}{filter} >>\nstream\n", body.len()).as_bytes());
        out.extend(&body);
        out.extend(b"\nendstream\nendobj\n%%EOF\n");
        out
    }

    #[test]
    fn reads_the_text_a_page_draws() {
        let content = b"BT /F1 12 Tf 72 720 Td (Invoice 2026-114) Tj 0 -16 Td [(Total:) -300 (1,234.50 EUR)] TJ T* (Due \\(net 30\\)) Tj ET";
        for compress in [false, true] {
            let t = extract(&pdf(content, compress)).unwrap();
            assert_eq!(t, "Invoice 2026-114\nTotal: 1,234.50 EUR\nDue (net 30)", "compress={compress}");
        }
    }

    #[test]
    fn hex_and_utf16_strings_are_decoded() {
        let t = extract(&pdf(b"BT <48656c6c6f20776f726c64> Tj T* <FEFF00E9007400E9> Tj ET", false)).unwrap();
        assert_eq!(t, "Hello world\nété");
    }

    #[test]
    fn reads_an_exported_pdf_through_its_fonts_unicode_maps() {
        // LibreOffice's export: subset fonts, glyph ids mapped by ToUnicode.
        let t = extract(include_bytes!("../tests/fixtures/q3-budget.pdf")).unwrap();
        assert!(t.contains("Q3 budget update"), "{t}");
        assert!(t.contains("5,500"), "{t}");
    }

    #[test]
    fn a_pdf_with_no_readable_text_says_so() {
        assert_eq!(extract(&pdf(b"q 100 0 0 100 0 0 cm /Im0 Do Q", false)), None);
        assert_eq!(extract(&pdf(b"BT <0102030405060708> Tj ET", false)), None);
        assert_eq!(extract(b"not a pdf"), None);
    }
}
