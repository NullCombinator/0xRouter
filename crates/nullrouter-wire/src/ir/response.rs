//! The non-stream response IR.

use super::FinishReason;
use super::event::{BlockKind, Event};
use super::request::Part;

/// A complete answer: the same content a stream carries, as final parts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Response {
    pub id: Option<String>,
    pub model: Option<String>,
    /// Text, thinking and tool-call parts, in order.
    pub content: Vec<Part>,
    pub usage: Option<Usage>,
    pub finish: Option<FinishReason>,
}

impl Response {
    /// All text parts joined.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|p| if let Part::Text { text, .. } = p { Some(text.as_str()) } else { None })
            .collect()
    }

    /// The stream events that carry this answer, for a client already streaming when a
    /// provider answers whole.
    pub fn events(&self) -> Vec<Event> {
        let mut out = vec![Event::Preamble { id: self.id.clone(), model: self.model.clone() }];
        for p in &self.content {
            match p {
                Part::Text { text, .. } => {
                    out.extend([Event::BlockStart(BlockKind::Text), Event::TextDelta(text.clone())])
                }
                Part::Thinking { text, signature, .. } => {
                    out.extend([Event::BlockStart(BlockKind::Thinking), Event::ThinkingDelta(text.clone())]);
                    out.extend(signature.clone().map(Event::Signature));
                }
                Part::ToolCall { id, name, arguments, .. } => out.extend([
                    Event::BlockStart(BlockKind::ToolCall { id: id.clone(), name: name.clone() }),
                    Event::ToolArguments(arguments.to_string()),
                ]),
                _ => continue,
            }
            out.push(Event::BlockStop);
        }
        out.extend(self.usage.map(Event::Usage));
        out.extend(self.finish.map(Event::Finish));
        out.push(Event::Done);
        out
    }
}

/// Token usage as reported. `input` excludes cache reads and writes; the style's
/// `input_semantics` says how to get there. `None` means "not reported" (SC-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
}

impl Usage {
    /// Later values win; used to merge usage reported in several events.
    pub fn merge(&mut self, other: Usage) {
        let pick = |a: &mut Option<u64>, b: Option<u64>| {
            if b.is_some() {
                *a = b;
            }
        };
        pick(&mut self.input, other.input);
        pick(&mut self.output, other.output);
        pick(&mut self.cache_read, other.cache_read);
        pick(&mut self.cache_write, other.cache_write);
        pick(&mut self.reasoning, other.reasoning);
    }

    pub fn is_empty(&self) -> bool {
        *self == Usage::default()
    }
}
