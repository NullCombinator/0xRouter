//! The generic request codec: a client body in any style → the IR → a wire body.
//!
//! Decoding is lenient: a part, tool or item no template matches goes to
//! [`Request::opaque`], and a key no rule reads goes to [`Request::unplaced`].
//!
//! A same-style attempt doesn't encode: [`forward`] sends the client's body with a few
//! named edits, so everything the client sent reaches the provider. A cross-style attempt
//! encodes from the IR: it refuses while anything is opaque, since content is never
//! dropped, and reports each unplaced key as [`Dropped`] (research R27).

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde_json::{Map, Value};
use zerorouter_registry::schema::{
    Alternation, ArgumentsForm, ContentForm, PartKind, ResponseFormatForm, StreamOn, SystemLayout, ThinkingForm,
    ToolCallsLayout, ToolResultContent, ToolResultMatch, ToolResultsLayout,
};
use zerorouter_registry::template::{FieldPath, PathSeg, Template};

use super::{CodecError, Dropped, PartTpl, Style, TextStyle};
use crate::ir::{
    Media, MediaSource, Message, Opaque, Part, Request, ResultContent, Role, Tool, ToolChoice,
};
use crate::primitives::{forms, media, repairs};
use crate::template::{Bindings, match_value, render, select_one, set_path, unmatched_keys};

/// The field that carries tool calls in the `message_field` layout.
const TOOL_CALLS_FIELD: &str = "tool_calls";

/// Part decode order: the most specific templates first.
const DECODE_ORDER: [PartKind; 6] =
    [PartKind::ToolCall, PartKind::ToolResult, PartKind::Image, PartKind::Audio, PartKind::Thinking, PartKind::Text];

/// Where a part goes in a style's message list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// Inside the message content.
    Inline,
    /// In the message's tool-calls field.
    Field,
    /// Its own item in the message list.
    Item,
}

pub(crate) fn place(t: &TextStyle, k: PartKind) -> Place {
    match k {
        PartKind::ToolCall => match t.layout.tool_calls {
            ToolCallsLayout::MessageField => Place::Field,
            ToolCallsLayout::OutputItem => Place::Item,
            ToolCallsLayout::ContentPart | ToolCallsLayout::FunctionCallPart => Place::Inline,
        },
        PartKind::ToolResult => match t.layout.tool_results {
            ToolResultsLayout::ToolRoleMessage | ToolResultsLayout::OutputItem => Place::Item,
            ToolResultsLayout::ContentPart | ToolResultsLayout::FunctionResponsePart => Place::Inline,
        },
        // Responses carries reasoning as its own item, like function calls.
        PartKind::Thinking if t.layout.tool_calls == ToolCallsLayout::OutputItem => Place::Item,
        _ => Place::Inline,
    }
}

/// The top-level field each system layout uses; `first_message` has none.
fn system_field(l: SystemLayout) -> Option<&'static str> {
    match l {
        SystemLayout::TopLevelField => Some("system"),
        SystemLayout::InstructionsField => Some("instructions"),
        SystemLayout::SystemInstruction => Some("systemInstruction"),
        SystemLayout::FirstMessage => None,
    }
}

/// The body paths a style's rules read, as a tree. A leaf takes its whole value.
#[derive(Debug, Default)]
struct Declared(BTreeMap<String, Option<Declared>>);

impl Declared {
    /// Adds `p` up to its first index or `[*]`, which then takes the whole value.
    fn add(&mut self, p: &FieldPath) {
        let keys: Vec<&str> = p.0.iter().map_while(|s| if let PathSeg::Key(k) = s { Some(k.as_str()) } else { None }).collect();
        let mut node = self;
        for (i, k) in keys.iter().enumerate() {
            let slot = node.0.entry((*k).to_owned()).or_insert_with(|| Some(Declared::default()));
            if i + 1 == keys.len() {
                *slot = None;
                return;
            }
            match slot {
                Some(child) => node = child,
                None => return,
            }
        }
    }

    fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Keys of `obj` no path reaches, as paths under `at`.
    fn unread(&self, obj: &Map<String, Value>, at: &str, out: &mut Vec<String>) {
        for (k, v) in obj {
            let here = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
            match (self.0.get(k), v) {
                (None, _) => out.push(here),
                (Some(Some(child)), Value::Object(o)) => child.unread(o, &here, out),
                _ => {}
            }
        }
    }
}

fn kind_name(k: PartKind) -> String {
    k.as_str().replace('_', " ")
}

// ── Decode ─────────────────────────────────────────────────────────────────────

/// Reads a client body written in `style`.
pub fn decode(style: &Style, body: &Value) -> Result<Request, CodecError> {
    let t = style.text()?;
    let obj = body.as_object().ok_or_else(|| CodecError::decode("", "the body is not a JSON object"))?;
    let mut d = Decoder::new(t, &style.id);
    let mut req = Request::default();

    if let Some(p) = &t.model_path {
        d.used(p);
        req.model = select_one(p, body).and_then(Value::as_str).unwrap_or_default().to_owned();
    }
    if let Some(p) = &t.stream_path {
        d.used(p);
        req.stream = select_one(p, body).and_then(Value::as_bool).unwrap_or(false);
    }
    if let Some(field) = system_field(t.layout.system) {
        d.declared.0.insert(field.to_owned(), None);
        if let Some(v) = obj.get(field) {
            req.system = d.system(v, field);
        }
    }
    d.used(&t.messages);
    match select_one(&t.messages, body) {
        Some(Value::Array(items)) => req.messages = d.messages(items),
        Some(Value::String(s)) => req.messages.push(Message { role: Role::User, parts: vec![Part::text(s.clone())] }),
        None | Some(Value::Null) => {}
        Some(_) => return Err(CodecError::decode(t.layout.messages.clone(), "not a list of messages")),
    }
    if t.layout.system == SystemLayout::FirstMessage {
        let lead = req.messages.iter().take_while(|m| m.role == Role::System).count();
        for m in req.messages.drain(..lead) {
            req.system.extend(m.parts);
        }
    }
    d.tools(body, &mut req);
    d.params(body, &mut req);

    req.extra = obj.iter().filter(|(k, _)| !d.declared.has(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    d.declared.unread(obj, "", &mut req.unplaced);
    req.unplaced.append(&mut d.unplaced);
    req.opaque = d.opaque;
    Ok(req)
}

pub(crate) struct Decoder<'a> {
    t: &'a TextStyle,
    style_id: &'a str,
    pub(crate) opaque: Vec<Opaque>,
    /// Keys inside messages, parts and tools that no template reads.
    unplaced: Vec<String>,
    declared: Declared,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(t: &'a TextStyle, style_id: &'a str) -> Self {
        Self { t, style_id, opaque: Vec::new(), unplaced: Vec::new(), declared: Declared::default() }
    }

    fn used(&mut self, p: &FieldPath) {
        self.declared.add(p);
    }

    fn keep(&mut self, at: String, v: &Value) {
        self.opaque.push(Opaque { at, value: v.clone() });
    }

    /// Records the keys of `v` that `t` (which matched it) doesn't read.
    fn leftovers(&mut self, t: &Template, v: &Value, at: &str) {
        unmatched_keys(t, v, at, &mut self.unplaced);
    }

    fn system(&mut self, v: &Value, at: &str) -> Vec<Part> {
        match v {
            Value::Object(o) if o.contains_key(&self.t.layout.content) => {
                self.content(&o[&self.t.layout.content], &format!("{at}.{}", self.t.layout.content))
            }
            other => self.content(other, at),
        }
    }

    fn messages(&mut self, items: &[Value]) -> Vec<Message> {
        let mut out: Vec<Message> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let at = format!("{}[{i}]", self.t.layout.messages);
            let Some((m, from_item)) = self.message(item, &at) else {
                self.keep(at, item);
                continue;
            };
            // Tool-call and tool-result items join the message before them.
            match out.last_mut() {
                Some(last) if from_item && last.role == m.role => last.parts.extend(m.parts),
                _ => out.push(m),
            }
        }
        out
    }

    fn role(&self, style_role: &str) -> Option<Role> {
        match self.t.role_in(style_role) {
            "developer" => Some(Role::System),
            r => Role::parse(r),
        }
    }

    pub(crate) fn message(&mut self, item: &Value, at: &str) -> Option<(Message, bool)> {
        let t = self.t;
        let l = &t.layout;
        if let Some((mt, b)) = t.message.as_ref().and_then(|mt| Some((mt, match_value(mt, item)?))) {
            let role = self.role(b.str("message.role")?)?;
            let parts = self.content(b.get("message.content").unwrap_or(&Value::Null), &format!("{at}.content"));
            self.leftovers(mt, item, at);
            return Some((Message { role, parts }, false));
        }
        if let Some(style_role) = item.get(&l.role).and_then(Value::as_str) {
            if l.tool_results == ToolResultsLayout::ToolRoleMessage && style_role == self.t.role_out("tool") {
                let (p, tpl) = self.part_match(PartKind::ToolResult, item)?;
                self.leftovers(tpl, item, at);
                return Some((Message { role: Role::Tool, parts: vec![p] }, true));
            }
            let role = self.role(style_role)?;
            let mut parts = self.content(item.get(&l.content).unwrap_or(&Value::Null), &format!("{at}.{}", l.content));
            let calls_field = l.tool_calls == ToolCallsLayout::MessageField;
            if calls_field && let Some(Value::Array(calls)) = item.get(TOOL_CALLS_FIELD) {
                for (j, c) in calls.iter().enumerate() {
                    let at = format!("{at}.{TOOL_CALLS_FIELD}[{j}]");
                    match self.part_match(PartKind::ToolCall, c) {
                        Some((p, tpl)) => {
                            self.leftovers(tpl, c, &at);
                            parts.push(p);
                        }
                        None => self.keep(at, c),
                    }
                }
            }
            if let Some(o) = item.as_object() {
                let read = |k: &str| k == l.role || k == l.content || (calls_field && k == TOOL_CALLS_FIELD);
                self.unplaced.extend(o.keys().filter(|k| !read(k)).map(|k| format!("{at}.{k}")));
            }
            return Some((Message { role, parts }, false));
        }
        if l.tool_calls == ToolCallsLayout::OutputItem
            && let Some((p, tpl)) = self.part_match(PartKind::ToolCall, item)
        {
            self.leftovers(tpl, item, at);
            return Some((Message { role: Role::Assistant, parts: vec![p] }, true));
        }
        if l.tool_results == ToolResultsLayout::OutputItem
            && let Some((p, tpl)) = self.part_match(PartKind::ToolResult, item)
        {
            self.leftovers(tpl, item, at);
            return Some((Message { role: Role::Tool, parts: vec![p] }, true));
        }
        if place(t, PartKind::Thinking) == Place::Item
            && let Some((p, tpl)) = self.part_match(PartKind::Thinking, item)
        {
            self.leftovers(tpl, item, at);
            return Some((Message { role: Role::Assistant, parts: vec![p] }, true));
        }
        None
    }

    pub(crate) fn content(&mut self, v: &Value, at: &str) -> Vec<Part> {
        match v {
            Value::Null => Vec::new(),
            Value::String(s) => vec![Part::text(s.clone())],
            Value::Array(items) => {
                let mut parts = Vec::new();
                for (j, el) in items.iter().enumerate() {
                    let at = format!("{at}[{j}]");
                    match self.inline_match(el) {
                        Some((p, tpl)) => {
                            self.leftovers(tpl, el, &at);
                            parts.push(p);
                        }
                        None => self.keep(at, el),
                    }
                }
                parts
            }
            other => {
                self.keep(at.to_owned(), other);
                Vec::new()
            }
        }
    }

    pub(crate) fn inline_part(&self, v: &Value) -> Option<Part> {
        self.inline_match(v).map(|(p, _)| p)
    }

    fn inline_match(&self, v: &Value) -> Option<(Part, &'a Template)> {
        DECODE_ORDER.iter().filter(|k| place(self.t, **k) == Place::Inline).find_map(|k| self.part_match(*k, v))
    }

    pub(crate) fn part_as(&self, kind: PartKind, v: &Value) -> Option<Part> {
        self.part_match(kind, v).map(|(p, _)| p)
    }

    /// The part and the template that matched it.
    fn part_match(&self, kind: PartKind, v: &Value) -> Option<(Part, &'a Template)> {
        let t: &'a TextStyle = self.t;
        let tpl = t.parts.get(&kind)?;
        tpl.matchers().find_map(|m| Some((self.build(kind, tpl, &match_value(m, v)?)?, m)))
    }

    fn build(&self, kind: PartKind, tpl: &PartTpl, b: &Bindings) -> Option<Part> {
        let cache_control = b.get("part.cache_control").cloned();
        Some(match kind {
            PartKind::Text => Part::Text { text: b.str("part.text")?.to_owned(), cache_control },
            PartKind::Image | PartKind::Audio => {
                let m = match (b.get("media"), tpl.codec) {
                    (Some(v), Some(codec)) => media::decode(codec, v)?,
                    _ => {
                        let mime = b.str("media.mime").map(str::to_owned);
                        match (b.str("media.data"), b.str("media.url")) {
                            (Some(d), _) => Media { mime, source: MediaSource::Base64(d.to_owned()) },
                            (None, Some(u)) => Media { mime, source: MediaSource::Url(u.to_owned()) },
                            (None, None) => return None,
                        }
                    }
                };
                if kind == PartKind::Image {
                    Part::Image { media: m, cache_control }
                } else {
                    Part::Audio { media: m, cache_control }
                }
            }
            PartKind::ToolCall => Part::ToolCall {
                id: b.str("call.id").unwrap_or_default().to_owned(),
                name: b.str("call.name")?.to_owned(),
                arguments: decode_arguments(self.t.layout.arguments, b.get("call.arguments")),
                cache_control,
            },
            PartKind::ToolResult => Part::ToolResult {
                id: b.str("result.id").unwrap_or_default().to_owned(),
                name: b.str("result.name").map(str::to_owned),
                content: self.result_content(b.get("result.content")),
                is_error: b.get("result.is_error").and_then(Value::as_bool).unwrap_or(false),
                cache_control,
            },
            PartKind::Thinking => Part::Thinking {
                text: b.str("part.text").unwrap_or_default().to_owned(),
                signature: b.str("part.signature").map(str::to_owned),
                vendor: Some(self.style_id.to_owned()),
            },
        })
    }

    fn result_content(&self, v: Option<&Value>) -> ResultContent {
        // The object form wraps a plain result as `{ result = … }`; unwrap it (9router
        // reads `response.result` the same way).
        if self.t.layout.tool_result_content == ToolResultContent::Object
            && let Some(Value::Object(o)) = v
            && let (1, Some(inner)) = (o.len(), o.get("result"))
            && !inner.is_object()
        {
            return match inner {
                Value::String(s) => ResultContent::Text(s.clone()),
                other => ResultContent::Json(other.clone()),
            };
        }
        match v {
            None | Some(Value::Null) => ResultContent::Text(String::new()),
            Some(Value::String(s)) => ResultContent::Text(s.clone()),
            Some(Value::Array(items)) => {
                let media_or_text = [PartKind::Text, PartKind::Image, PartKind::Audio];
                let parts: Option<Vec<Part>> =
                    items.iter().map(|el| media_or_text.iter().find_map(|k| self.part_as(*k, el))).collect();
                parts.map_or_else(|| ResultContent::Json(Value::Array(items.clone())), ResultContent::Parts)
            }
            Some(other) => ResultContent::Json(other.clone()),
        }
    }

    fn tools(&mut self, body: &Value, req: &mut Request) {
        let t = self.t;
        let Some(tt) = &t.tools else { return };
        self.used(&tt.path);
        if let Some(v) = select_one(&tt.path, body) {
            let at = tt.path.to_string();
            let mut defs: Vec<(String, Value)> = Vec::new();
            let mut unwrap = |d: &mut Self, w: &Template, at: String, el: &Value| {
                match match_value(w, el).as_ref().and_then(|b| b.get("tools.list")) {
                    Some(Value::Array(list)) => {
                        d.leftovers(w, el, &at);
                        defs.extend(list.iter().enumerate().map(|(i, t)| (format!("{at}.list[{i}]"), t.clone())));
                    }
                    _ => d.keep(at, el),
                }
            };
            match (&tt.wrap, v) {
                // A one-element list template wraps each element (gemini `functionDeclarations`).
                (Some(Template::Array(each)), Value::Array(items)) if each.len() == 1 => {
                    for (i, el) in items.iter().enumerate() {
                        unwrap(self, &each[0], format!("{at}[{i}]"), el);
                    }
                }
                (Some(wrap), _) => unwrap(self, wrap, at.clone(), v),
                (None, Value::Array(items)) => {
                    defs.extend(items.iter().enumerate().map(|(i, d)| (format!("{at}[{i}]"), d.clone())));
                }
                (None, other) => self.keep(at.clone(), other),
            }
            for (a, def) in defs {
                match match_value(&tt.data, &def).and_then(|b| tool_of(&b)) {
                    Some(tool) => {
                        self.leftovers(&tt.data, &def, &a);
                        req.tools.push(tool);
                    }
                    None => self.keep(a, &def),
                }
            }
        }
        let Some(c) = &tt.choice else { return };
        self.used(&c.path);
        let Some(v) = select_one(&c.path, body) else { return };
        let fixed = [(Some(&c.auto), ToolChoice::Auto), (Some(&c.required), ToolChoice::Required), (c.none.as_ref(), ToolChoice::None)];
        let choice = fixed
            .into_iter()
            .find_map(|(tpl, ch)| Some((ch, tpl.filter(|tpl| match_value(tpl, v).is_some())?)))
            .or_else(|| {
                let name = match_value(&c.named, v)?.str("tool.name")?.to_owned();
                Some((ToolChoice::Named(name), &c.named))
            });
        match choice {
            Some((ch, tpl)) => {
                self.leftovers(tpl, v, &c.path.to_string());
                req.tool_choice = Some(ch);
            }
            None => self.keep(c.path.to_string(), v),
        }
    }

    fn params(&mut self, body: &Value, req: &mut Request) {
        let t = self.t;
        for p in &t.params {
            self.used(&p.path);
            let Some(v) = select_one(&p.path, body) else { continue };
            match p.name.as_str() {
                "thinking" => {
                    let form = p.form.as_deref().and_then(ThinkingForm::parse);
                    req.params.thinking = form.and_then(|f| forms::decode_thinking(f, v));
                }
                "response_format" => {
                    let form = p.form.as_deref().and_then(ResponseFormatForm::parse);
                    req.params.response_format = form.and_then(|f| forms::decode_response_format(f, v));
                }
                name => {
                    req.params.values.insert(name.to_owned(), v.clone());
                }
            }
        }
    }
}

fn tool_of(b: &Bindings) -> Option<Tool> {
    Some(Tool {
        name: b.str("tool.name")?.to_owned(),
        description: b.str("tool.description").map(str::to_owned),
        parameters: b.get("tool.parameters").cloned().unwrap_or_else(empty_schema),
        cache_control: b.get("tool.cache_control").cloned(),
    })
}

fn empty_schema() -> Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

fn decode_arguments(form: ArgumentsForm, v: Option<&Value>) -> Value {
    match (form, v) {
        (_, None | Some(Value::Null)) => Value::Object(Map::new()),
        (ArgumentsForm::JsonString, Some(Value::String(s))) if s.trim().is_empty() => Value::Object(Map::new()),
        (ArgumentsForm::JsonString, Some(Value::String(s))) => {
            serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone()))
        }
        (_, Some(v)) => v.clone(),
    }
}

// ── Encode ─────────────────────────────────────────────────────────────────────

/// What a same-style attempt changes in the client's body (research R27).
#[derive(Debug, Clone, Copy, Default)]
pub struct Edits<'a> {
    /// The upstream model id, written at the style's model path.
    pub model: Option<&'a str>,
    /// The stream flag, when the endpoint forces streaming.
    pub stream: Option<bool>,
    /// Turns on the request switch of the style's usage stream event (Chat's
    /// `stream_options.include_usage`), so a streamed answer reports usage (R13). Set it
    /// on streamed attempts only.
    pub include_usage: bool,
}

/// The client's body for a same-style attempt: as received, with `edits` at their named
/// paths only. Unknown keys and content no template describes go through unchanged.
pub fn forward(body: &Value, wire: &Style, edits: &Edits) -> Result<Value, CodecError> {
    let t = wire.text()?;
    let mut out = body.clone();
    if let (Some(p), Some(m)) = (&t.model_path, edits.model) {
        set_path(&mut out, p, Value::String(m.to_owned()));
    }
    if let (Some(p), Some(s)) = (&t.stream_path, edits.stream) {
        set_path(&mut out, p, Value::Bool(s));
    }
    if edits.include_usage {
        let switches = t.events.iter().filter(|e| e.on == Some(StreamOn::Usage)).filter_map(|e| e.when_request.as_ref());
        for p in switches {
            set_path(&mut out, p, Value::Bool(true));
        }
    }
    Ok(out)
}

/// A wire body, and the client's keys it couldn't carry.
#[derive(Debug, Clone, PartialEq)]
pub struct Encoded {
    pub body: Value,
    pub dropped: Vec<Dropped>,
}

/// Writes `req` as a `wire` body. `client_style` is the style the client used; the wire's
/// repairs run only when it differs.
///
/// Refuses while anything is opaque. Each unplaced key comes back as [`Dropped`]: across
/// styles all of them, since no rule of the client's style read them; in the client's own
/// style only the nested ones, since top-level keys are copied (use [`forward`] instead).
pub fn encode(req: &Request, wire: &Style, client_style: &str) -> Result<Encoded, CodecError> {
    let t = wire.text()?;
    if let Some(o) = req.opaque.first() {
        return Err(CodecError::carry(o.at.clone(), "the client sent content no template of its style describes"));
    }
    let same = client_style == wire.id;
    let reason = if same {
        "an encode copies only top-level keys the style doesn't read".to_owned()
    } else {
        format!("no {client_style} rule reads it, so it has no place in {}", wire.id)
    };
    let dropped = req
        .unplaced
        .iter()
        .filter(|p| !(same && req.extra.contains_key(p.as_str())))
        .map(|p| Dropped { path: p.clone(), reason: reason.clone() })
        .collect();
    let mut req = Cow::Borrowed(req);
    if !same && !t.repairs.is_empty() {
        let r = req.to_mut();
        for rep in &t.repairs {
            repairs::apply(*rep, r);
        }
    }
    let e = Encoder { t, wire_id: &wire.id, call_names: call_names(&req.messages) };
    let mut out = Value::Object(if same { req.extra.clone() } else { Map::new() });

    if let Some(p) = &t.model_path {
        set_path(&mut out, p, Value::String(req.model.clone()));
    }
    if let Some(p) = &t.stream_path {
        set_path(&mut out, p, Value::Bool(req.stream));
    }

    let (system, messages) = e.split_system(&req)?;
    if let (Some(field), false) = (system_field(t.layout.system), system.is_empty()) {
        let v = e.system(&system)?;
        out[field] = v;
    }
    let items = e.messages(&messages)?;
    set_path(&mut out, &t.messages, Value::Array(items));
    e.tools(&req, &mut out)?;
    e.params(&req, &mut out)?;
    Ok(Encoded { body: out, dropped })
}

fn call_names(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| match p {
            Part::ToolCall { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect()
}

pub(crate) struct Encoder<'a> {
    t: &'a TextStyle,
    wire_id: &'a str,
    call_names: BTreeMap<String, String>,
}

impl<'a> Encoder<'a> {
    pub(crate) fn new(t: &'a TextStyle, wire_id: &'a str) -> Self {
        Self { t, wire_id, call_names: BTreeMap::new() }
    }

    /// The system parts for the wire's system field, and the messages left to encode.
    fn split_system<'r>(&self, req: &'r Request) -> Result<(Vec<Part>, Cow<'r, [Message]>), CodecError> {
        let mut system = req.system.clone();
        let mut messages: Cow<'r, [Message]> = Cow::Borrowed(&req.messages);
        match self.t.layout.system {
            SystemLayout::FirstMessage => {
                if !system.is_empty() {
                    let mut all = vec![Message { role: Role::System, parts: std::mem::take(&mut system) }];
                    all.extend(req.messages.iter().cloned());
                    messages = Cow::Owned(all);
                }
            }
            SystemLayout::InstructionsField => {
                // `instructions` is one string; anything else goes in as a system message.
                let one_text = matches!(system.as_slice(), [Part::Text { cache_control: None, .. }]);
                if !system.is_empty() && !one_text {
                    let mut all = vec![Message { role: Role::System, parts: std::mem::take(&mut system) }];
                    all.extend(req.messages.iter().cloned());
                    messages = Cow::Owned(all);
                }
            }
            SystemLayout::TopLevelField | SystemLayout::SystemInstruction => {
                let lead = req.messages.iter().take_while(|m| m.role == Role::System).count();
                if lead > 0 {
                    system.extend(req.messages[..lead].iter().flat_map(|m| m.parts.iter().cloned()));
                    messages = Cow::Borrowed(&req.messages[lead..]);
                }
                if messages.iter().any(|m| m.role == Role::System) {
                    return Err(CodecError::carry(
                        "system message",
                        "the wire takes system text only before the conversation, not between turns",
                    ));
                }
            }
        }
        Ok((system, messages))
    }

    fn system(&self, parts: &[Part]) -> Result<Value, CodecError> {
        match self.t.layout.system {
            SystemLayout::InstructionsField => Ok(Value::String(parts[0].text_str().unwrap_or_default().to_owned())),
            SystemLayout::SystemInstruction => {
                let rendered = self.inline(parts, &self.t.role_out("system"))?;
                let mut o = Map::new();
                o.insert(self.t.layout.content.clone(), Value::Array(rendered));
                Ok(Value::Object(o))
            }
            _ => self.content(parts, &self.t.role_out("system"), false),
        }
    }

    fn inline(&self, parts: &[Part], role: &str) -> Result<Vec<Value>, CodecError> {
        parts.iter().filter(|p| self.carries_thinking(p)).map(|p| self.part(p, role)).collect()
    }

    /// Message content under the style's content form.
    fn content(&self, parts: &[Part], role: &str, has_fields: bool) -> Result<Value, CodecError> {
        let parts: Vec<&Part> = parts.iter().filter(|p| self.carries_thinking(p)).collect();
        Ok(match parts.as_slice() {
            [] if has_fields => Value::Null,
            [] if self.t.layout.content_form == ContentForm::StringWhenTextOnly => Value::String(String::new()),
            [Part::Text { text, cache_control: None }] if self.t.layout.content_form == ContentForm::StringWhenTextOnly => {
                Value::String(text.clone())
            }
            ps => Value::Array(ps.iter().map(|p| self.part(p, role)).collect::<Result<_, _>>()?),
        })
    }

    /// Thinking goes only to the vendor that produced it, and only if the wire has a
    /// thinking template (research R4); signatures are vendor-specific.
    pub(crate) fn carries_thinking(&self, p: &Part) -> bool {
        match p {
            Part::Thinking { vendor, .. } => {
                self.t.parts.contains_key(&PartKind::Thinking) && vendor.as_deref().is_none_or(|v| v == self.wire_id)
            }
            _ => true,
        }
    }

    fn style_role(&self, r: Role) -> String {
        match r {
            Role::Tool if place(self.t, PartKind::ToolResult) == Place::Inline => {
                self.t.layout.roles.get("tool").cloned().unwrap_or_else(|| "user".into())
            }
            r => self.t.role_out(r.as_str()),
        }
    }

    fn messages(&self, messages: &[Message]) -> Result<Vec<Value>, CodecError> {
        // Adjacent messages of the same wire role merge first, at the IR level.
        let mut units: Vec<(String, Vec<&Part>)> = Vec::new();
        for m in messages {
            let role = self.style_role(m.role);
            match units.last_mut() {
                Some((r, parts)) if self.t.layout.alternation == Alternation::MergeAdjacent && *r == role => {
                    parts.extend(&m.parts);
                }
                _ => units.push((role, m.parts.iter().collect())),
            }
        }
        let mut out = Vec::new();
        for (role, parts) in units {
            self.unit(&role, &parts, &mut out)?;
        }
        Ok(out)
    }

    pub(crate) fn unit(&self, role: &str, parts: &[&Part], out: &mut Vec<Value>) -> Result<(), CodecError> {
        let mut inline: Vec<Part> = Vec::new();
        let mut fields: Vec<Value> = Vec::new();
        let mut emitted = false;
        for p in parts.iter().copied().filter(|p| self.carries_thinking(p)) {
            match place(self.t, p.kind()) {
                Place::Inline => inline.push(p.clone()),
                Place::Field => fields.push(self.part(p, role)?),
                Place::Item => {
                    if !inline.is_empty() || !fields.is_empty() {
                        out.push(self.message(role, &std::mem::take(&mut inline), std::mem::take(&mut fields))?);
                    }
                    out.push(self.part(p, role)?);
                    emitted = true;
                }
            }
        }
        if !inline.is_empty() || !fields.is_empty() || !emitted {
            out.push(self.message(role, &inline, fields)?);
        }
        Ok(())
    }

    fn message(&self, role: &str, inline: &[Part], fields: Vec<Value>) -> Result<Value, CodecError> {
        let content = self.content(inline, role, !fields.is_empty())?;
        let mut v = match &self.t.message {
            Some(mt) => render(mt, &Bindings::new().with("message.role", role).with("message.content", content)),
            None => {
                let mut o = Map::new();
                o.insert(self.t.layout.role.clone(), Value::String(role.to_owned()));
                o.insert(self.t.layout.content.clone(), content);
                Value::Object(o)
            }
        };
        if !fields.is_empty()
            && let Some(o) = v.as_object_mut()
        {
            o.insert(TOOL_CALLS_FIELD.into(), Value::Array(fields));
        }
        Ok(v)
    }

    pub(crate) fn part(&self, p: &Part, role: &str) -> Result<Value, CodecError> {
        let kind = p.kind();
        let tpl = self
            .t
            .parts
            .get(&kind)
            .ok_or_else(|| CodecError::carry(kind_name(kind), "the wire style has no template for it"))?;
        let mut b = Bindings::new();
        if let Some(c) = p.cache_control() {
            b.set("part.cache_control", c.clone());
        }
        match p {
            Part::Text { text, .. } => b.set("part.text", text.as_str()),
            Part::Image { media: m, .. } | Part::Audio { media: m, .. } => {
                if let Some(codec) = tpl.codec {
                    let v = media::encode(codec, m).ok_or_else(|| {
                        CodecError::carry(kind_name(kind), format!("the wire's {} codec can't carry inline data", codec.as_str()))
                    })?;
                    b.set("media", v);
                }
                for (k, v) in media::fields(m) {
                    if let Some(v) = v {
                        b.set(k, v);
                    }
                }
            }
            Part::ToolCall { id, name, arguments, .. } => {
                b.set("call.id", id.as_str());
                b.set("call.name", name.as_str());
                b.set("call.arguments", encode_arguments(self.t.layout.arguments, arguments));
            }
            Part::ToolResult { id, name, content, is_error, .. } => {
                b.set("result.id", id.as_str());
                let name = name.clone().or_else(|| self.call_names.get(id).cloned());
                match name {
                    Some(n) => b.set("result.name", n),
                    None if self.t.layout.tool_result_match == ToolResultMatch::Name => {
                        return Err(CodecError::carry(
                            "tool result",
                            format!("the wire matches results by name and no call has id {id:?}"),
                        ));
                    }
                    None => {}
                }
                b.set("result.content", self.result_content(content, role)?);
                b.set("result.is_error", *is_error);
            }
            Part::Thinking { text, signature, .. } => {
                b.set("part.text", text.as_str());
                if let Some(s) = signature {
                    b.set("part.signature", s.as_str());
                }
            }
        }
        Ok(render(tpl.for_role(role), &b))
    }

    fn result_content(&self, c: &ResultContent, role: &str) -> Result<Value, CodecError> {
        let form = self.t.layout.tool_result_content;
        let only_text = |ps: &[Part]| match ps {
            [Part::Text { text, .. }] => Some(text.clone()),
            _ => None,
        };
        let parts = |ps: &[Part]| -> Result<Value, CodecError> {
            Ok(Value::Array(ps.iter().map(|p| self.part(p, role)).collect::<Result<_, _>>()?))
        };
        let json_text = |v: &Value| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        Ok(match (form, c) {
            (ToolResultContent::Object, ResultContent::Json(v)) if v.is_object() => v.clone(),
            (ToolResultContent::Object, ResultContent::Json(v)) => serde_json::json!({ "result": v }),
            (ToolResultContent::Object, ResultContent::Text(s)) => serde_json::json!({ "result": s }),
            (ToolResultContent::Object | ToolResultContent::String, ResultContent::Parts(ps)) => match only_text(ps) {
                Some(s) if form == ToolResultContent::String => Value::String(s),
                Some(s) => serde_json::json!({ "result": s }),
                None => {
                    return Err(CodecError::carry(
                        "tool result",
                        "the wire takes a tool result as one text, and this one has several parts or media",
                    ));
                }
            },
            (ToolResultContent::String | ToolResultContent::StringOrParts, ResultContent::Text(s)) => Value::String(s.clone()),
            (ToolResultContent::String | ToolResultContent::StringOrParts, ResultContent::Json(v)) => {
                Value::String(json_text(v))
            }
            (ToolResultContent::StringOrParts | ToolResultContent::Parts, ResultContent::Parts(ps)) => parts(ps)?,
            (ToolResultContent::Parts, ResultContent::Text(s)) => parts(&[Part::text(s.clone())])?,
            (ToolResultContent::Parts, ResultContent::Json(v)) => parts(&[Part::text(json_text(v))])?,
        })
    }

    fn tools(&self, req: &Request, out: &mut Value) -> Result<(), CodecError> {
        if req.tools.is_empty() {
            return match req.tool_choice {
                None | Some(ToolChoice::Auto | ToolChoice::None) => Ok(()),
                Some(_) => Err(CodecError::carry("tool choice", "the request forces a tool but declares none")),
            };
        }
        let tt = self.t.tools.as_ref().ok_or_else(|| CodecError::carry("tools", "the wire style takes no tools"))?;
        let defs: Vec<Value> = req
            .tools
            .iter()
            .map(|tool| {
                let mut b = Bindings::new()
                    .with("tool.name", tool.name.as_str())
                    .with("tool.parameters", tool.parameters.clone());
                if let Some(d) = &tool.description {
                    b.set("tool.description", d.as_str());
                }
                if let Some(c) = &tool.cache_control {
                    b.set("tool.cache_control", c.clone());
                }
                render(&tt.data, &b)
            })
            .collect();
        let list = match &tt.wrap {
            Some(w) => render(w, &Bindings::new().with("tools.list", Value::Array(defs))),
            None => Value::Array(defs),
        };
        set_path(out, &tt.path, list);
        let Some(choice) = &req.tool_choice else { return Ok(()) };
        let Some(c) = &tt.choice else {
            return match choice {
                ToolChoice::Auto => Ok(()),
                _ => Err(CodecError::carry("tool choice", "the wire style has no tool-choice field")),
            };
        };
        let v = match choice {
            ToolChoice::Auto => render(&c.auto, &Bindings::new()),
            ToolChoice::Required => render(&c.required, &Bindings::new()),
            ToolChoice::None => match &c.none {
                Some(n) => render(n, &Bindings::new()),
                None => return Err(CodecError::carry("tool choice", "the wire style can't turn tools off")),
            },
            ToolChoice::Named(n) => render(&c.named, &Bindings::new().with("tool.name", n.as_str())),
        };
        set_path(out, &c.path, v);
        Ok(())
    }

    fn params(&self, req: &Request, out: &mut Value) -> Result<(), CodecError> {
        let declared = |name: &str| self.t.params.iter().any(|p| p.name == name);
        if req.params.response_format.is_some() && !declared("response_format") {
            return Err(CodecError::carry("response format", "the wire style has no structured-output field"));
        }
        for p in &self.t.params {
            let v = match p.name.as_str() {
                "thinking" => {
                    let (Some(t), Some(form)) = (&req.params.thinking, p.form.as_deref().and_then(ThinkingForm::parse))
                    else {
                        continue;
                    };
                    forms::encode_thinking(form, t)
                }
                "response_format" => {
                    let (Some(rf), Some(form)) =
                        (&req.params.response_format, p.form.as_deref().and_then(ResponseFormatForm::parse))
                    else {
                        continue;
                    };
                    Some(forms::encode_response_format(form, rf))
                }
                name => req.params.values.get(name).cloned(),
            };
            if let Some(v) = v {
                merge_at(out, &p.path, v);
            }
        }
        Ok(())
    }
}

/// Sets `v` at `path`, merging objects so two params can share a parent object.
fn merge_at(out: &mut Value, path: &FieldPath, v: Value) {
    let merged = match (select_one(path, out), v) {
        (Some(Value::Object(old)), Value::Object(new)) => {
            let mut o = old.clone();
            o.extend(new);
            Value::Object(o)
        }
        (_, v) => v,
    };
    set_path(out, path, merged);
}

fn encode_arguments(form: ArgumentsForm, v: &Value) -> Value {
    match (form, v) {
        (ArgumentsForm::JsonString, Value::String(s)) => Value::String(s.clone()),
        (ArgumentsForm::JsonString, v) => Value::String(v.to_string()),
        (ArgumentsForm::JsonObject, Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| v.clone()),
        (ArgumentsForm::JsonObject, v) => v.clone(),
    }
}

impl Part {
    fn text_str(&self) -> Option<&str> {
        match self {
            Part::Text { text, .. } => Some(text),
            _ => None,
        }
    }
}
