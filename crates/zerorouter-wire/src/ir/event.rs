//! The stream event IR (research R3). One provider stream decodes to these; one client
//! stream is encoded from them.

use serde_json::Value;

use super::response::Usage;
use super::FinishReason;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Response metadata, before any content.
    Preamble { id: Option<String>, model: Option<String> },
    BlockStart(BlockKind),
    TextDelta(String),
    ThinkingDelta(String),
    Signature(String),
    /// A fragment of the open tool call's JSON arguments.
    ToolArguments(String),
    BlockStop,
    Usage(Usage),
    Finish(FinishReason),
    Error(ErrorEvent),
    Keepalive,
    Done,
}

impl Event {
    /// Whether this event carries answer content (the R9 `output_seen` threshold).
    pub fn is_output(&self) -> bool {
        matches!(
            self,
            Event::BlockStart(_)
                | Event::TextDelta(_)
                | Event::ThinkingDelta(_)
                | Event::Signature(_)
                | Event::ToolArguments(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Thinking,
    ToolCall { id: String, name: String },
}

/// An error reported inside a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorEvent {
    pub status: Option<u16>,
    pub kind: Option<String>,
    pub message: String,
    /// The provider's frame, for records only.
    pub raw: Option<Value>,
}
