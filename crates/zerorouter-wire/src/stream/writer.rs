//! IR events → a client stream in the client's style (data-model § ClientStreamState).
//!
//! Finish and usage are held until `Done`, so a style that reports them together
//! (Anthropic `message_delta`) sees the final usage whatever order the provider used.
//! Under `tool_arguments = whole` a tool call is written once, when its block closes.

use serde_json::{Map, Value};
use zerorouter_registry::schema::{FinishReason, Framing, StreamOn, ToolArgumentsMode};
use zerorouter_registry::template::FieldPath;

use crate::codec::response::{client_parts, encode as encode_response, prefixed_id};
use crate::codec::{CodecError, EventTpl, Style, TextStyle};
use crate::ir::{BlockKind, ErrorEvent, Event, Part, Response, Usage};
use crate::template::{Bindings, Chain, render, select_one};
use crate::usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenBlock {
    #[default]
    None,
    Text { index: u64 },
    Thinking { index: u64, signed: bool },
    ToolCall { index: u64, args_started: bool },
}

/// What the client has been sent so far. The counters carry across segments when a
/// broken stream is continued or restarted.
#[derive(Debug, Clone, Default)]
pub struct ClientStreamState {
    pub preamble_sent: bool,
    pub output_seen: bool,
    pub open_block: OpenBlock,
    pub next_block_index: u64,
    pub next_output_index: u64,
    pub sequence_number: u64,
    /// Text of the current answer, a copy kept for continuation.
    pub partial_text: String,
    pub usage: Usage,
}

/// The block being written.
#[derive(Debug, Clone, Default)]
struct Block {
    id: String,
    name: String,
    text: String,
    arguments: String,
    signature: Option<String>,
    output_index: u64,
    ordinal: u64,
}

pub struct StreamWriter<'s> {
    style: &'s Style,
    t: &'s TextStyle,
    pub state: ClientStreamState,
    request: Value,
    id: String,
    model: String,
    created: u64,
    block: Block,
    tools_started: u64,
    answer: Vec<Part>,
    finish: Option<FinishReason>,
    array_started: bool,
    ended: bool,
}

impl<'s> StreamWriter<'s> {
    /// `request` is the client's body, read by `when_request` rules.
    pub fn new(client: &'s Style, request: &Value, id: &str, model: &str, created: u64) -> Result<Self, CodecError> {
        let t = client.text()?;
        Ok(Self {
            style: client,
            t,
            state: ClientStreamState::default(),
            request: request.clone(),
            id: prefixed_id(&t.id_prefix, id),
            model: model.to_owned(),
            created,
            block: Block::default(),
            tools_started: 0,
            answer: Vec::new(),
            finish: None,
            array_started: false,
            ended: false,
        })
    }

    /// The client bytes for one IR event (often empty).
    pub fn write(&mut self, ev: &Event) -> String {
        let mut out = String::new();
        if self.ended {
            return out;
        }
        if !matches!(ev, Event::Preamble { .. } | Event::Keepalive | Event::Error(_)) {
            self.preamble(&mut out);
        }
        match ev {
            Event::Preamble { model, .. } => {
                if let Some(m) = model.as_ref().filter(|_| self.model.is_empty()) {
                    self.model.clone_from(m);
                }
                self.preamble(&mut out);
            }
            Event::BlockStart(kind) => self.start(kind, &mut out),
            Event::TextDelta(s) => {
                if !matches!(self.state.open_block, OpenBlock::Text { .. }) {
                    self.start(&BlockKind::Text, &mut out);
                }
                self.block.text.push_str(s);
                self.state.partial_text.push_str(s);
                self.state.output_seen = true;
                self.emit(StreamOn::TextDelta, &Bindings::new().with("delta.text", s.as_str()), &mut out);
            }
            Event::ThinkingDelta(s) => {
                if !matches!(self.state.open_block, OpenBlock::Thinking { .. }) {
                    self.start(&BlockKind::Thinking, &mut out);
                }
                self.block.text.push_str(s);
                self.state.output_seen = true;
                self.emit(StreamOn::ThinkingDelta, &Bindings::new().with("delta.thinking", s.as_str()), &mut out);
            }
            Event::Signature(s) => {
                self.block.signature = Some(s.clone());
                if let OpenBlock::Thinking { signed, .. } = &mut self.state.open_block {
                    *signed = true;
                }
                self.emit(StreamOn::Signature, &Bindings::new().with("delta.signature", s.as_str()), &mut out);
            }
            Event::ToolArguments(s) => {
                let OpenBlock::ToolCall { args_started, .. } = &mut self.state.open_block else { return out };
                *args_started = true;
                self.block.arguments.push_str(s);
                self.state.output_seen = true;
                if self.t.tool_arguments == ToolArgumentsMode::Fragments {
                    self.emit(StreamOn::ToolArguments, &Bindings::new().with("delta.arguments", s.as_str()), &mut out);
                }
            }
            Event::BlockStop => self.stop(&mut out),
            Event::Usage(u) => self.state.usage.merge(*u),
            Event::Finish(r) => self.finish = Some(*r),
            Event::Error(e) => self.error(e, &mut out),
            Event::Keepalive => self.keepalive(&mut out),
            Event::Done => self.done(&mut out),
        }
        out
    }

    /// Ends the client stream if `Done` never came.
    pub fn end(&mut self) -> String {
        let mut out = String::new();
        if !self.ended {
            self.preamble(&mut out);
            self.done(&mut out);
        }
        out
    }

    /// The answer so far, as the non-stream response IR.
    pub fn response(&self) -> Response {
        let mut content = self.answer.clone();
        if let Some(p) = self.open_part() {
            content.push(p);
        }
        Response {
            id: Some(self.id.clone()),
            model: Some(self.model.clone()),
            content,
            usage: (!self.state.usage.is_empty()).then_some(self.state.usage),
            finish: self.finish,
        }
    }

    fn preamble(&mut self, out: &mut String) {
        if !self.state.preamble_sent {
            self.state.preamble_sent = true;
            self.emit(StreamOn::Preamble, &Bindings::new(), out);
        }
    }

    fn start(&mut self, kind: &BlockKind, out: &mut String) {
        if self.state.open_block != OpenBlock::None {
            self.stop(out);
        }
        let index = self.state.next_block_index;
        self.state.next_block_index += 1;
        self.block = Block { output_index: self.state.next_output_index, ordinal: self.tools_started, ..Block::default() };
        self.state.next_output_index += 1;
        let on = match kind {
            BlockKind::Text => {
                self.state.open_block = OpenBlock::Text { index };
                StreamOn::BlockStartText
            }
            BlockKind::Thinking => {
                self.state.open_block = OpenBlock::Thinking { index, signed: false };
                StreamOn::BlockStartThinking
            }
            BlockKind::ToolCall { id, name } => {
                self.state.open_block = OpenBlock::ToolCall { index, args_started: false };
                self.block.id = if id.is_empty() { format!("call_{}_{index}", self.id) } else { id.clone() };
                self.block.name.clone_from(name);
                self.tools_started += 1;
                self.state.output_seen = true;
                StreamOn::BlockStartToolCall
            }
        };
        self.emit(on, &Bindings::new(), out);
    }

    fn stop(&mut self, out: &mut String) {
        let (specific, is_tool) = match self.state.open_block {
            OpenBlock::None => return,
            OpenBlock::Text { .. } => (StreamOn::BlockStopText, false),
            OpenBlock::Thinking { .. } => (StreamOn::BlockStopThinking, false),
            OpenBlock::ToolCall { .. } => (StreamOn::BlockStopToolCall, true),
        };
        if is_tool && self.t.tool_arguments == ToolArgumentsMode::Whole {
            self.emit(StreamOn::ToolArguments, &Bindings::new(), out);
        }
        if self.has(specific) {
            self.emit(specific, &Bindings::new(), out);
        } else {
            self.emit(StreamOn::BlockStop, &Bindings::new(), out);
        }
        if let Some(p) = self.open_part() {
            self.answer.push(p);
        }
        self.state.open_block = OpenBlock::None;
    }

    fn open_part(&self) -> Option<Part> {
        let b = &self.block;
        Some(match self.state.open_block {
            OpenBlock::None => return None,
            OpenBlock::Text { .. } => Part::text(b.text.clone()),
            OpenBlock::Thinking { .. } => Part::Thinking {
                text: b.text.clone(),
                signature: b.signature.clone(),
                vendor: Some(self.style.id.clone()),
            },
            OpenBlock::ToolCall { .. } => Part::ToolCall {
                id: b.id.clone(),
                name: b.name.clone(),
                arguments: arguments_json(&b.arguments),
                cache_control: None,
            },
        })
    }

    fn done(&mut self, out: &mut String) {
        self.stop(out);
        let finish = self.finish.unwrap_or(if self.answer.iter().any(|p| matches!(p, Part::ToolCall { .. })) {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        });
        self.finish = Some(finish);
        self.emit(StreamOn::Finish, &Bindings::new(), out);
        if !self.state.usage.is_empty() {
            self.emit(StreamOn::Usage, &Bindings::new(), out);
        }
        self.emit(StreamOn::Done, &Bindings::new(), out);
        match self.t.framing {
            Framing::SseDataDone => out.push_str("data: [DONE]\n\n"),
            Framing::JsonArray => out.push_str(if self.array_started { "]" } else { "[]" }),
            _ => {}
        }
        self.ended = true;
    }

    fn error(&mut self, e: &ErrorEvent, out: &mut String) {
        let status = e.status.unwrap_or(500);
        let kind = e.kind.clone().unwrap_or_else(|| self.style.error_type(status).to_owned());
        let mut b = Bindings::new()
            .with("error.type", kind)
            .with("error.message", e.message.as_str())
            .with("error.status", status)
            .with("error.details", Value::Object(Map::new()));
        let body = render(&self.style.error_body, &b);
        b.set("error.body", body);
        let tpl = self.style.error_event.clone();
        self.frame(&tpl, &b, out);
    }

    fn keepalive(&mut self, out: &mut String) {
        match self.style.keepalive.clone() {
            Some(tpl) => self.frame(&tpl, &Bindings::new(), out),
            None if matches!(self.t.framing, Framing::SseNamed | Framing::SseData | Framing::SseDataDone) => {
                out.push_str(": keepalive\n\n");
            }
            None => {}
        }
    }

    fn has(&self, on: StreamOn) -> bool {
        self.t.events.iter().any(|e| e.on == Some(on))
    }

    /// Writes every template for `on` whose `when_request` holds.
    fn emit(&mut self, on: StreamOn, extra: &Bindings, out: &mut String) {
        let t = self.t;
        for tpl in t.events.iter().filter(|e| e.on == Some(on)) {
            if let Some(p) = &tpl.when_request
                && !truthy(&self.request, p)
            {
                continue;
            }
            self.frame(tpl, extra, out);
        }
    }

    fn frame(&mut self, tpl: &EventTpl, extra: &Bindings, out: &mut String) {
        let mut base = self.bindings();
        if tpl.data.mentions("response.rendered")
            && let Ok(v) = encode_response(self.style, &self.rendered(), self.created)
        {
            base.set("response.rendered", v);
        }
        let data = render(&tpl.data, &Chain(extra, &base));
        self.state.sequence_number += 1;
        let json = data.to_string();
        match self.t.framing {
            Framing::SseNamed => {
                if let Some(name) = &tpl.event {
                    out.push_str("event: ");
                    out.push_str(name);
                    out.push('\n');
                }
                out.push_str("data: ");
                out.push_str(&json);
                out.push_str("\n\n");
            }
            Framing::SseData | Framing::SseDataDone => {
                out.push_str("data: ");
                out.push_str(&json);
                out.push_str("\n\n");
            }
            Framing::Ndjson => {
                out.push_str(&json);
                out.push('\n');
            }
            Framing::JsonArray => {
                out.push(if self.array_started { ',' } else { '[' });
                self.array_started = true;
                out.push_str(&json);
            }
        }
    }

    fn rendered(&self) -> Response {
        let mut r = self.response();
        r.content = client_parts(self.style, &r.content);
        r
    }

    fn bindings(&self) -> Bindings {
        let b = &self.block;
        let index = match self.state.open_block {
            OpenBlock::Text { index } | OpenBlock::Thinking { index, .. } | OpenBlock::ToolCall { index, .. } => index,
            OpenBlock::None => self.state.next_block_index.saturating_sub(1),
        };
        let mut out = Bindings::new()
            .with("response.id", self.id.as_str())
            .with("response.model", self.model.as_str())
            .with("response.created", self.created)
            .with("block.index", index)
            .with("block.full_text", b.text.as_str())
            .with("block.full_arguments", b.arguments.as_str())
            .with("block.arguments_json", arguments_json(&b.arguments))
            .with("tool.ordinal", b.ordinal)
            .with("output.index", b.output_index)
            .with("sequence.number", self.state.sequence_number);
        if !b.id.is_empty() {
            out.set("block.id", b.id.as_str());
            out.set("block.name", b.name.as_str());
        }
        if let Some(s) = &b.signature {
            out.set("block.signature", s.as_str());
        }
        if let Some(f) = self.finish {
            out.set("finish", self.t.finish_from_ir(f));
        }
        usage::bind(self.t.usage.semantics, &self.state.usage, &mut out);
        out
    }
}

fn arguments_json(s: &str) -> Value {
    if s.trim().is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_owned()))
}

fn truthy(body: &Value, p: &FieldPath) -> bool {
    matches!(select_one(p, body), Some(v) if !matches!(v, Value::Null | Value::Bool(false)))
}
