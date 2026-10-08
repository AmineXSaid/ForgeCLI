//! Fold a stream of [`StreamEvent`]s into a complete [`ApiMessage`].

use forge_types::{ApiMessage, ContentBlock, Delta, Role, StreamEvent, Usage};
use serde_json::Value;

use crate::ApiError;

/// The key a tool input gets when its arguments weren't valid JSON at the end of its block:
/// cut off by `max_tokens`, or written invalid. The engine tells the two apart by stop reason.
pub const TRUNCATED_INPUT: &str = "_truncated_input";
/// The arguments as received, next to [`TRUNCATED_INPUT`] (up to [`MAX_RAW_INPUT`] bytes).
pub const RAW_INPUT: &str = "_raw_input";
/// The key the engine gives a call the model ended itself with invalid, unrepairable arguments.
pub const INVALID_INPUT: &str = "_invalid_input";
/// Raw arguments longer than this keep only their end, where the mistake usually is.
pub const MAX_RAW_INPUT: usize = 16 * 1024;

/// Builds the final message while events arrive; tool inputs arrive as
/// partial JSON and are parsed when their block stops.
#[derive(Debug, Default)]
pub struct MessageAccumulator {
    message: Option<ApiMessage>,
    partial_json: Vec<String>,
}

impl MessageAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: &StreamEvent) -> Result<(), ApiError> {
        match event {
            StreamEvent::MessageStart { message } => {
                self.message = Some(message.clone());
                self.partial_json.clear();
            }
            StreamEvent::ContentBlockStart { index, content_block } => {
                let msg = self.msg()?;
                while msg.content.len() < *index {
                    msg.content.push(ContentBlock::text(""));
                }
                if msg.content.len() == *index {
                    msg.content.push(content_block.clone());
                } else {
                    msg.content[*index] = content_block.clone();
                }
                while self.partial_json.len() <= *index {
                    self.partial_json.push(String::new());
                }
                self.partial_json[*index].clear();
            }
            StreamEvent::ContentBlockDelta { index, delta } => {
                let index = *index;
                while self.partial_json.len() <= index {
                    self.partial_json.push(String::new());
                }
                if let Delta::InputJsonDelta { partial_json } = delta {
                    self.partial_json[index].push_str(partial_json);
                    return Ok(());
                }
                let msg = self.msg()?;
                let block = msg
                    .content
                    .get_mut(index)
                    .ok_or_else(|| ApiError::Parse(format!("delta for unknown block {index}")))?;
                match (block, delta) {
                    (ContentBlock::Text { text, .. }, Delta::TextDelta { text: t }) => text.push_str(t),
                    (ContentBlock::Text { citations, .. }, Delta::CitationsDelta { citation }) => {
                        let list = citations.get_or_insert_with(|| Value::Array(vec![]));
                        if let Value::Array(a) = list {
                            a.push(citation.clone());
                        }
                    }
                    (ContentBlock::Thinking { thinking, .. }, Delta::ThinkingDelta { thinking: t }) => {
                        thinking.push_str(t)
                    }
                    (ContentBlock::Thinking { signature, .. }, Delta::SignatureDelta { signature: s }) => {
                        signature.push_str(s)
                    }
                    _ => {}
                }
            }
            StreamEvent::ContentBlockStop { index } => {
                let json = self.partial_json.get(*index).cloned().unwrap_or_default();
                let msg = self.msg()?;
                if let Some(ContentBlock::ToolUse { input, .. } | ContentBlock::ServerToolUse { input, .. }) =
                    msg.content.get_mut(*index)
                {
                    if !json.trim().is_empty() {
                        // Cut off mid-input (usually `max_tokens`): keep the call, marked, so the
                        // engine can answer it with an error instead of failing the whole turn.
                        *input = serde_json::from_str(&json).unwrap_or_else(|e| {
                            let mut from = json.len().saturating_sub(MAX_RAW_INPUT);
                            while !json.is_char_boundary(from) {
                                from += 1;
                            }
                            serde_json::json!({
                                TRUNCATED_INPUT: format!("incomplete JSON ({e}), {} bytes", json.len()),
                                RAW_INPUT: &json[from..],
                            })
                        });
                    } else if input.is_null() {
                        *input = Value::Object(Default::default());
                    }
                }
            }
            StreamEvent::MessageDelta { delta, usage } => {
                let msg = self.msg()?;
                msg.stop_reason = delta.stop_reason;
                msg.stop_sequence = delta.stop_sequence.clone();
                merge_usage(&mut msg.usage, usage);
            }
            StreamEvent::MessageStop | StreamEvent::Ping => {}
            StreamEvent::Error { error } => {
                return Err(ApiError::Stream { kind: error.kind.clone(), message: error.message.clone() })
            }
        }
        Ok(())
    }

    fn msg(&mut self) -> Result<&mut ApiMessage, ApiError> {
        self.message.as_mut().ok_or_else(|| ApiError::Parse("event before message_start".into()))
    }

    /// The message built so far (complete after `message_stop`).
    pub fn snapshot(&self) -> Option<&ApiMessage> {
        self.message.as_ref()
    }

    pub fn finish(self) -> Result<ApiMessage, ApiError> {
        let mut m = self.message.ok_or_else(|| ApiError::Parse("stream ended without a message".into()))?;
        m.role = Role::Assistant;
        // Drop empty text blocks the API never sent (padding from out-of-order starts).
        m.content.retain(|b| !matches!(b, ContentBlock::Text { text, citations: None, .. } if text.is_empty()));
        Ok(m)
    }
}

/// `message_delta.usage` carries cumulative counts; non-zero values win.
fn merge_usage(into: &mut Usage, from: &Usage) {
    if from.input_tokens > 0 {
        into.input_tokens = from.input_tokens;
    }
    if from.output_tokens > 0 {
        into.output_tokens = from.output_tokens;
    }
    if from.cache_creation_input_tokens > 0 {
        into.cache_creation_input_tokens = from.cache_creation_input_tokens;
    }
    if from.cache_read_input_tokens > 0 {
        into.cache_read_input_tokens = from.cache_read_input_tokens;
    }
    if from.server_tool_use.is_some() {
        into.server_tool_use = from.server_tool_use.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::events_for;
    use forge_types::StopReason;
    use serde_json::json;

    #[test]
    fn rebuilds_text_and_tool_use() {
        let msg = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![
                ContentBlock::text("Looking."),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: json!({"file_path":"/a"}),
                    cache_control: None,
                },
            ],
            stop_reason: Some(StopReason::ToolUse),
            stop_sequence: None,
            usage: Usage { input_tokens: 10, output_tokens: 7, ..Default::default() },
        };
        let mut acc = MessageAccumulator::new();
        for ev in events_for(&msg) {
            acc.push(&ev).unwrap();
        }
        let out = acc.finish().unwrap();
        assert_eq!(out.content, msg.content);
        assert_eq!(out.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(out.usage.output_tokens, 7);
    }

    #[test]
    fn invalid_tool_json_is_marked_truncated() {
        let mut acc = MessageAccumulator::new();
        let start: StreamEvent = serde_json::from_value(json!({"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"x","content":[],"stop_reason":null,"usage":{}}})).unwrap();
        acc.push(&start).unwrap();
        acc.push(&serde_json::from_value(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"Bash","input":{}}})).unwrap()).unwrap();
        acc.push(&serde_json::from_value(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\": "}})).unwrap()).unwrap();
        acc.push(&serde_json::from_value(json!({"type":"content_block_stop","index":0})).unwrap()).unwrap();
        let ContentBlock::ToolUse { input, .. } = &acc.snapshot().unwrap().content[0] else { panic!() };
        assert!(input[TRUNCATED_INPUT].as_str().unwrap().contains("incomplete JSON"), "{input}");
        assert_eq!(input[RAW_INPUT], "{\"command\": ", "the raw arguments are kept");
    }
}
