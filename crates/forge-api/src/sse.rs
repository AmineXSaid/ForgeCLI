//! A minimal server-sent-events decoder.

/// One decoded event: the `event:` name (if any) and the joined `data:` lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser. Feed raw bytes; take complete events out.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: String,
    pending: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push bytes and return every event completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        // Keep incomplete UTF-8 sequences until the rest arrives.
        self.pending.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        let chunk = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
        self.pending.drain(..valid);
        self.buf.push_str(&chunk.replace("\r\n", "\n").replace('\r', "\n"));

        let mut out = Vec::new();
        while let Some(pos) = self.buf.find("\n\n") {
            let raw: String = self.buf.drain(..pos + 2).collect();
            if let Some(ev) = parse_block(&raw) {
                out.push(ev);
            }
        }
        out
    }

    /// Flush a trailing event that was not terminated by a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let raw = std::mem::take(&mut self.buf);
        parse_block(&raw)
    }
}

fn parse_block(raw: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in raw.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() && event.is_none() {
        return None;
    }
    Some(SseEvent { event, data: data.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_across_chunks() {
        let mut d = SseDecoder::new();
        assert!(d.push(b"event: ping\ndata: {\"type\"").is_empty());
        let evs = d.push(b":\"ping\"}\n\nevent: message_stop\r\ndata: {}\r\n\r\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event.as_deref(), Some("ping"));
        assert_eq!(evs[0].data, "{\"type\":\"ping\"}");
        assert_eq!(evs[1].event.as_deref(), Some("message_stop"));
    }

    #[test]
    fn handles_split_utf8_and_comments() {
        let mut d = SseDecoder::new();
        let s = "data: é\n\n".as_bytes();
        assert!(d.push(&s[..7]).is_empty());
        let evs = d.push(&s[7..]);
        assert_eq!(evs[0].data, "é");
        assert!(d.push(b": keepalive\n\n").is_empty());
    }

    #[test]
    fn multi_line_data_joined() {
        let mut d = SseDecoder::new();
        let evs = d.push(b"data: a\ndata: b\n\n");
        assert_eq!(evs[0].data, "a\nb");
    }
}
