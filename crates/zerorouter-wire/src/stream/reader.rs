//! Provider frames → IR events, by reverse-matching the wire style's stream templates.
//!
//! Every template that matches a frame contributes, in declaration order (an OpenAI chunk
//! can carry a text delta, a finish reason and usage at once). Under `blocks = implicit`
//! the reader opens and closes blocks itself, so the IR stream always has explicit blocks.

use serde_json::Value;
use zerorouter_registry::schema::{BlockModel, StreamOn, ToolArgumentsMode};

use super::Frame;
use crate::codec::{CodecError, Style, TextStyle};
use crate::ir::{BlockKind, ErrorEvent, Event};
use crate::template::{Bindings, match_all};
use crate::usage;

#[derive(Debug, Clone, PartialEq)]
enum Open {
    Text,
    Thinking,
    /// A tool call, keyed by the provider's ordinal when it sends one.
    Tool(Option<Value>),
}

pub struct StreamReader<'s> {
    t: &'s TextStyle,
    preamble_seen: bool,
    open: Option<Open>,
    done: bool,
}

impl<'s> StreamReader<'s> {
    pub fn new(wire: &'s Style) -> Result<Self, CodecError> {
        Ok(Self { t: wire.text()?, preamble_seen: false, open: None, done: false })
    }

    /// True once the provider signalled the end of the stream.
    pub fn saw_done(&self) -> bool {
        self.done
    }

    pub fn read(&mut self, frame: &Frame) -> Result<Vec<Event>, CodecError> {
        let mut out = Vec::new();
        if frame.is_done() {
            self.close(&mut out);
            self.done = true;
            out.push(Event::Done);
            return Ok(out);
        }
        let data: Value = serde_json::from_str(&frame.data)
            .map_err(|e| CodecError::decode("stream frame", format!("not JSON: {e}")))?;
        let t = self.t;
        let mut matched = false;
        for tpl in &t.events {
            if let (Some(want), Some(got)) = (&tpl.event, &frame.event)
                && want != got
            {
                continue;
            }
            let Some(on) = tpl.on else { continue };
            for b in match_all(&tpl.matcher, &data) {
                matched = true;
                self.apply(on, &b, &data, &mut out);
            }
        }
        if !matched && let Some(err) = data.get("error").filter(|e| e.is_object()) {
            out.push(Event::Error(ErrorEvent {
                status: None,
                kind: err.get("type").and_then(Value::as_str).map(str::to_owned),
                message: err.get("message").and_then(Value::as_str).unwrap_or("upstream stream error").to_owned(),
                raw: Some(data.clone()),
            }));
        }
        Ok(out)
    }

    /// Closes a block left open when the provider's stream ends.
    pub fn finish(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        self.close(&mut out);
        out
    }

    fn close(&mut self, out: &mut Vec<Event>) {
        if self.open.take().is_some() {
            out.push(Event::BlockStop);
        }
    }

    fn ensure(&mut self, want: Open, kind: BlockKind, out: &mut Vec<Event>) {
        if self.open.as_ref() != Some(&want) {
            self.close(out);
            self.open = Some(want);
            out.push(Event::BlockStart(kind));
        }
    }

    fn apply(&mut self, on: StreamOn, b: &Bindings, data: &Value, out: &mut Vec<Event>) {
        let whole = self.t.tool_arguments == ToolArgumentsMode::Whole;
        let implicit = self.t.blocks == BlockModel::Implicit;
        match on {
            StreamOn::Preamble => {
                if !self.preamble_seen {
                    self.preamble_seen = true;
                    out.push(Event::Preamble {
                        id: b.str("response.id").map(str::to_owned),
                        model: b.str("response.model").map(str::to_owned),
                    });
                }
            }
            StreamOn::BlockStartText => {
                self.close(out);
                self.open = Some(Open::Text);
                out.push(Event::BlockStart(BlockKind::Text));
            }
            StreamOn::BlockStartThinking => {
                self.close(out);
                self.open = Some(Open::Thinking);
                out.push(Event::BlockStart(BlockKind::Thinking));
            }
            StreamOn::BlockStartToolCall => {
                self.start_tool(b, out);
                self.whole_arguments(b, whole && implicit, out);
            }
            StreamOn::TextDelta => {
                if let Some(s) = b.str("delta.text").filter(|s| !s.is_empty()) {
                    self.ensure(Open::Text, BlockKind::Text, out);
                    out.push(Event::TextDelta(s.to_owned()));
                }
            }
            StreamOn::ThinkingDelta => {
                if let Some(s) = b.str("delta.thinking").filter(|s| !s.is_empty()) {
                    self.ensure(Open::Thinking, BlockKind::Thinking, out);
                    out.push(Event::ThinkingDelta(s.to_owned()));
                }
            }
            StreamOn::Signature => {
                if let Some(s) = b.str("delta.signature").or_else(|| b.str("block.signature")) {
                    out.push(Event::Signature(s.to_owned()));
                }
            }
            StreamOn::ToolArguments => {
                let ordinal = b.get("tool.ordinal").cloned();
                let same = matches!(&self.open, Some(Open::Tool(o)) if ordinal.is_none() || *o == ordinal);
                if !same || b.get("block.name").is_some() && implicit && whole {
                    self.start_tool(b, out);
                }
                if let Some(s) = b.str("delta.arguments").filter(|s| !s.is_empty()) {
                    out.push(Event::ToolArguments(s.to_owned()));
                }
                self.whole_arguments(b, whole && implicit, out);
            }
            StreamOn::BlockStop | StreamOn::BlockStopText | StreamOn::BlockStopThinking | StreamOn::BlockStopToolCall => {
                self.close(out);
            }
            StreamOn::Usage => {}
            StreamOn::Finish => {
                if implicit {
                    self.close(out);
                }
                if let Some(r) = b.str("finish") {
                    out.push(Event::Finish(self.t.finish_to_ir(r)));
                }
            }
            StreamOn::Error => out.push(Event::Error(ErrorEvent {
                status: None,
                kind: b.str("error.type").map(str::to_owned),
                message: b.str("error.message").unwrap_or("upstream stream error").to_owned(),
                raw: Some(data.clone()),
            })),
            StreamOn::Keepalive => out.push(Event::Keepalive),
            StreamOn::Done => {
                self.close(out);
                self.done = true;
                out.push(Event::Done);
            }
        }
        let u = usage::read(&self.t.usage, (on == StreamOn::Usage).then_some(data), b);
        if !u.is_empty() {
            // Usage belongs before the finish it came with.
            let at = out.iter().rposition(|e| matches!(e, Event::Finish(_))).unwrap_or(out.len());
            out.insert(at, Event::Usage(u));
        }
    }

    fn start_tool(&mut self, b: &Bindings, out: &mut Vec<Event>) {
        self.close(out);
        self.open = Some(Open::Tool(b.get("tool.ordinal").cloned()));
        out.push(Event::BlockStart(BlockKind::ToolCall {
            id: b.str("block.id").unwrap_or_default().to_owned(),
            name: b.str("block.name").unwrap_or_default().to_owned(),
        }));
    }

    /// Whole arguments (`{block.arguments_json}`): one fragment, and under implicit blocks
    /// the call is complete.
    fn whole_arguments(&mut self, b: &Bindings, closes: bool, out: &mut Vec<Event>) {
        let Some(v) = b.get("block.arguments_json") else { return };
        out.push(Event::ToolArguments(match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }));
        if closes {
            self.close(out);
        }
    }
}
