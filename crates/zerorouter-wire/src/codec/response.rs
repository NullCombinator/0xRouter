//! The generic response codec: a provider body → the [`Response`] IR → a client body.
//! Across styles the second hop goes through the IR; in one style the provider's body
//! goes to the client as received (research R27), and the IR is only read from it.

use serde_json::Value;
use zerorouter_registry::schema::{FinishReason, PartKind};

use super::request::{Decoder, Encoder, Place, place};
use super::{CodecError, Style};
use crate::ir::{Part, Response};
use crate::template::{Bindings, match_value, render};
use crate::usage;

/// Reads a provider's non-stream body written in `wire`'s style.
pub fn decode(wire: &Style, body: &Value) -> Result<Response, CodecError> {
    let t = wire.text()?;
    let b = match_value(&t.response, body)
        .ok_or_else(|| CodecError::decode("", format!("the body doesn't have the {} response shape", wire.id)))?;
    let mut d = Decoder::new(t, &wire.id);
    let mut content = Vec::new();
    if let Some(s) = b.str("response.thinking") {
        content.push(Part::Thinking { text: s.to_owned(), signature: None, vendor: Some(wire.id.clone()) });
    }
    if let Some(Value::Array(items)) = b.get("response.content") {
        for item in items {
            if let Some(p) = d.inline_part(item) {
                content.push(p);
            } else if let Some((m, _)) = d.message(item, "response.content") {
                content.extend(m.parts);
            }
            // Anything else (server-side tool items) carries no answer content.
        }
    }
    if let Some(s) = b.str("response.text") {
        content.push(Part::text(s));
    }
    if let Some(Value::Array(calls)) = b.get("response.tool_calls") {
        content.extend(calls.iter().filter_map(|c| d.part_as(PartKind::ToolCall, c)));
    }
    let has_calls = content.iter().any(|p| matches!(p, Part::ToolCall { .. }));
    let finish = match (b.str("response.finish"), b.str("response.status")) {
        (Some(r), _) => t.finish_to_ir(r),
        (None, Some("incomplete")) => FinishReason::Length,
        (None, _) if has_calls => FinishReason::ToolCalls,
        (None, _) => FinishReason::Stop,
    };
    let u = usage::read(&t.usage, Some(body), &b);
    Ok(Response {
        id: b.str("response.id").map(str::to_owned),
        model: b.str("response.model").map(str::to_owned),
        content,
        usage: (!u.is_empty()).then_some(u),
        finish: Some(finish),
    })
}

/// A provider's non-stream answer, ready for the client.
#[derive(Debug, Clone, PartialEq)]
pub enum ForClient {
    /// Same style: send the provider's bytes as received. `read` is the IR (usage, finish)
    /// when the body has the style's response shape.
    AsReceived { read: Option<Response> },
    /// Across styles: `body` rebuilt in the client's style from `read`.
    Rebuilt { read: Response, body: Value },
}

/// Prepares the provider's non-stream `body`, written in `wire`'s style, for a `client`.
pub fn for_client(client: &Style, wire: &Style, body: &Value, created: u64) -> Result<ForClient, CodecError> {
    if client.id == wire.id {
        return Ok(ForClient::AsReceived { read: decode(wire, body).ok() });
    }
    let read = decode(wire, body)?;
    let body = encode(client, &read, created)?;
    Ok(ForClient::Rebuilt { read, body })
}

/// `id` with the style's prefix, unless it already has it.
pub fn prefixed_id(prefix: &str, id: &str) -> String {
    if id.starts_with(prefix) { id.to_owned() } else { format!("{prefix}{id}") }
}

/// Writes `r` in `client`'s style. `created` is the Unix time the caller stamps.
pub fn encode(client: &Style, r: &Response, created: u64) -> Result<Value, CodecError> {
    let t = client.text()?;
    let e = Encoder::new(t, &client.id);
    let role = t.role_out("assistant");
    let parts = client_parts(client, &r.content);
    let finish = r.finish.unwrap_or(if parts.iter().any(|p| matches!(p, Part::ToolCall { .. })) {
        FinishReason::ToolCalls
    } else {
        FinishReason::Stop
    });

    let mut b = Bindings::new()
        .with("response.id", prefixed_id(&t.id_prefix, r.id.as_deref().unwrap_or_default()))
        .with("response.model", r.model.clone().unwrap_or_default())
        .with("response.created", created)
        .with("response.status", if finish == FinishReason::Length { "incomplete" } else { "completed" })
        .with("response.finish", t.finish_from_ir(finish));

    let content = if place(t, PartKind::ToolCall) == Place::Item || t.message.is_some() {
        let mut items = Vec::new();
        if !parts.is_empty() {
            e.unit(&role, &parts.iter().collect::<Vec<_>>(), &mut items)?;
        }
        items
    } else {
        let inline = parts.iter().filter(|p| place(t, p.kind()) == Place::Inline);
        inline.map(|p| e.part(p, &role)).collect::<Result<_, _>>()?
    };
    b.set("response.content", Value::Array(content));

    let joined = |f: fn(&Part) -> Option<&str>| {
        let found: Vec<&str> = parts.iter().filter_map(f).collect();
        (!found.is_empty()).then(|| found.concat())
    };
    if let Some(text) = joined(|p| if let Part::Text { text, .. } = p { Some(text) } else { None }) {
        b.set("response.text", text);
    }
    if let Some(text) = joined(|p| if let Part::Thinking { text, .. } = p { Some(text) } else { None }) {
        b.set("response.thinking", text);
    }
    let calls: Vec<Value> = parts
        .iter()
        .filter(|p| matches!(p, Part::ToolCall { .. }))
        .map(|p| e.part(p, &role))
        .collect::<Result<_, _>>()?;
    if !calls.is_empty() {
        b.set("response.tool_calls", Value::Array(calls));
    }
    if let Some(u) = &r.usage {
        usage::bind(t.usage.semantics, u, &mut b);
    }
    Ok(render(&t.response, &b))
}

/// The answer parts a client style shows. Thinking needs a template; a signature from
/// another vendor is left out, since the client could only send it back to the wrong one.
pub(crate) fn client_parts(client: &Style, content: &[Part]) -> Vec<Part> {
    let has_thinking = client.text.as_ref().is_some_and(|t| t.parts.contains_key(&PartKind::Thinking));
    content
        .iter()
        .filter_map(|p| match p {
            Part::Thinking { .. } if !has_thinking => None,
            Part::Thinking { text, signature, vendor } => Some(Part::Thinking {
                text: text.clone(),
                signature: signature.clone().filter(|_| vendor.as_deref().is_none_or(|v| v == client.id)),
                vendor: Some(client.id.clone()),
            }),
            other => Some(other.clone()),
        })
        .collect()
}
