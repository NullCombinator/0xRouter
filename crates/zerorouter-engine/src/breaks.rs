//! Mid-stream breaks: continuation, restart, error event (research R9).
//!
//! A stream that breaks after content reached the client is resumed by the next attempt:
//! it continues the cut answer where the target declares `[continuation]` and can take
//! it, else restarts the answer after a note, or ends with an error event, as the agent
//! key (else the operator) chose. A break while a tool call is being sent always ends
//! with the error event: no style can take back a half-sent call.

use serde_json::Value;
use zerorouter_registry::schema::{ContinuationMethod, ContinuationUnless};
use zerorouter_wire::codec::request::{self, Edits};
use zerorouter_wire::codec::{Dropped, Style};
use zerorouter_wire::ir::{BlockKind, Event, Message, Part, Request, Role};
use zerorouter_wire::template::{select_one, set_path};

use crate::attempt::TextRequest;
use crate::plan::Candidate;

/// The block the client has open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Open {
    #[default]
    None,
    Text,
    Thinking,
    ToolCall,
}

/// What the client has been sent of the current answer.
#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    pub open: Open,
    /// The answer's text, the prefill of a continuation.
    pub text: String,
    /// The answer holds a thinking block or a tool call, which a prefill can't carry.
    pub other: bool,
    pub tool_call: bool,
}

impl Seen {
    pub fn see(&mut self, ev: &Event) {
        match ev {
            Event::BlockStart(BlockKind::Text) => self.open = Open::Text,
            Event::BlockStart(BlockKind::Thinking) | Event::ThinkingDelta(_) => {
                self.open = Open::Thinking;
                self.other = true;
            }
            Event::BlockStart(BlockKind::ToolCall { .. }) => {
                self.open = Open::ToolCall;
                self.other = true;
                self.tool_call = true;
            }
            Event::TextDelta(s) => {
                self.open = Open::Text;
                self.text.push_str(s);
            }
            Event::BlockStop => self.open = Open::None,
            _ => {}
        }
    }
}

/// How the attempt after a break resumes the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resume {
    Continue,
    Restart,
}

/// A break after output, until an attempt resumes the answer.
#[derive(Debug, Clone)]
pub(crate) struct Broken {
    pub reason: String,
    /// Set when an attempt starts; applied at its first content.
    pub resume: Option<Resume>,
}

/// The body that continues the cut answer on `c`: the original request with the answer so
/// far as the trailing assistant turn. `Err` says why `c` can't continue it.
pub(crate) fn continuation(
    req: &TextRequest,
    c: &Candidate<'_>,
    wire: &Style,
    seen: &Seen,
) -> Result<(Value, Vec<Dropped>), String> {
    let p = &c.provider.id;
    let Some(decl) = &c.endpoint.continuation else { return Err(format!("{p} declares no continuation")) };
    let model = &c.upstream_id;
    if !decl.models.is_empty() && !decl.models.contains(model) || decl.except_models.contains(model) {
        return Err(format!("{p} doesn't continue {model}"));
    }
    for u in &decl.unless {
        let holds = match u {
            ContinuationUnless::ThinkingEnabled => req.ir.params.thinking.as_ref().is_some_and(|t| t.enabled),
            ContinuationUnless::ToolCallInProgress => seen.tool_call,
        };
        if holds {
            return Err(format!("{p} doesn't continue when {}", u.as_str()));
        }
    }
    if seen.other {
        return Err("the cut answer holds more than text".into());
    }
    let text = if decl.trim_trailing_whitespace { seen.text.trim_end() } else { seen.text.as_str() };
    if text.trim().is_empty() {
        return Err("the cut answer has no text".into());
    }
    let turn = Message { role: Role::Assistant, parts: vec![Part::text(text)] };
    let carry = |e: zerorouter_wire::codec::CodecError| format!("the cut answer can't be carried to {p}: {e}");
    let t = wire.text().map_err(carry)?;
    let (mut body, dropped) = if c.same_style(&req.client.id) {
        // The client's own body, so an optimizer's fields survive the continuation (R10).
        let edits = Edits { model: Some(model), stream: c.endpoint.force_stream.then_some(true), include_usage: true };
        let mut body = request::forward(&req.body, wire, &edits).map_err(carry)?;
        let one = Request { messages: vec![turn], ..Request::default() };
        let enc = request::encode(&one, wire, &wire.id).map_err(carry)?;
        let Some(Value::Array(added)) = select_one(&t.messages, &enc.body).cloned() else {
            return Err(format!("{p}'s wire has no message list"));
        };
        let mut list = match select_one(&t.messages, &body) {
            Some(Value::Array(a)) => a.clone(),
            _ => return Err("the request has no message list".into()),
        };
        list.extend(added);
        set_path(&mut body, &t.messages, Value::Array(list));
        (body, Vec::new())
    } else {
        let mut ir = req.ir.clone();
        ir.model.clone_from(model);
        ir.stream = true;
        ir.messages.push(turn);
        let enc = request::encode(&ir, wire, &req.client.id).map_err(carry)?;
        let body =
            request::forward(&enc.body, wire, &Edits { include_usage: true, ..Edits::default() }).map_err(carry)?;
        (body, enc.dropped)
    };
    if decl.method == ContinuationMethod::PrefixFlag {
        let mut list = select_one(&t.messages, &body).cloned();
        if let Some(Value::Object(last)) = list.as_mut().and_then(Value::as_array_mut).and_then(|a| a.last_mut()) {
            last.insert("prefix".into(), Value::Bool(true));
        }
        if let Some(list) = list {
            set_path(&mut body, &t.messages, list);
        }
    }
    Ok((body, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_follows_the_open_block_and_the_text() {
        let mut s = Seen::default();
        for ev in [Event::BlockStart(BlockKind::Text), Event::TextDelta("Hel".into()), Event::TextDelta("lo ".into())] {
            s.see(&ev);
        }
        assert_eq!((s.open, s.text.as_str(), s.other), (Open::Text, "Hello ", false));
        s.see(&Event::BlockStop);
        s.see(&Event::BlockStart(BlockKind::ToolCall { id: "t".into(), name: "f".into() }));
        assert_eq!((s.open, s.other, s.tool_call), (Open::ToolCall, true, true));
    }
}
