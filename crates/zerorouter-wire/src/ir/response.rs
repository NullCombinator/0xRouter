//! The non-stream response IR.

use super::request::Part;
use super::FinishReason;

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
        self.content.iter().filter_map(|p| if let Part::Text { text, .. } = p { Some(text.as_str()) } else { None }).collect()
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
