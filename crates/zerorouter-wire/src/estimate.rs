//! 9router's input-token estimator (research R15).
//!
//! `estimateAnthropicInputTokens`: ceil(chars / 4) over a Messages body's system prompt,
//! tool definitions and message content. Characters are UTF-16 code units, as JavaScript
//! counts them. A request in another style is translated into the Messages shape first.

use serde_json::Value;

use crate::codec::CodecError;
use crate::codec::request::{self, Encoded};
use crate::codec::style::Style;
use crate::ir::Request;

/// The estimate for `req`, written as a `messages` (Messages-wire) body first.
pub fn estimate(req: &Request, messages: &Style, client_style: &str) -> Result<u64, CodecError> {
    let Encoded { body, .. } = request::encode(req, messages, client_style)?;
    Ok(messages_body(&body))
}

/// The estimate for a Messages-shaped body.
pub fn messages_body(body: &Value) -> u64 {
    let mut chars = value_chars(body.get("system")) + value_chars(body.get("tools"));
    if let Some(Value::Array(messages)) = body.get("messages") {
        chars += messages.iter().map(message_chars).sum::<u64>();
    }
    chars.div_ceil(4)
}

fn len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

fn value_chars(v: Option<&Value>) -> u64 {
    match v {
        None | Some(Value::Null) => 0,
        Some(Value::String(s)) => len(s),
        Some(Value::Bool(b)) => len(&b.to_string()),
        Some(Value::Number(n)) => len(&js_number(n)),
        Some(Value::Array(items)) => items.iter().map(|i| value_chars(Some(i))).sum(),
        Some(Value::Object(m)) => m.iter().map(|(k, i)| len(k) + value_chars(Some(i))).sum(),
    }
}

/// `String(n)` in JavaScript: integral floats print without a fraction.
fn js_number(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(f) if !n.is_i64() && !n.is_u64() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        _ => n.to_string(),
    }
}

fn block_chars(block: &Value) -> u64 {
    let Value::Object(b) = block else {
        return value_chars(Some(block));
    };
    match b.get("type").and_then(Value::as_str) {
        Some("text") => value_chars(b.get("text")),
        Some("tool_use") => value_chars(b.get("name")) + value_chars(b.get("input")),
        Some("tool_result") => value_chars(b.get("content")),
        Some("thinking") => value_chars(b.get("thinking")),
        _ => value_chars(Some(block)),
    }
}

fn message_chars(message: &Value) -> u64 {
    match message.get("content") {
        Some(Value::String(s)) => len(s),
        Some(Value::Array(blocks)) => blocks.iter().map(block_chars).sum(),
        other if message.is_object() => value_chars(other),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_utf16_units_and_js_numbers() {
        // "héllo 世界 👋🏽" is 13 UTF-16 units (the emoji and its modifier are 2 each): 4 tokens.
        assert_eq!(messages_body(&json!({"messages": [{"role": "user", "content": "héllo 世界 👋🏽"}]})), 4);
        assert_eq!(js_number(&serde_json::Number::from_f64(2.0).unwrap()), "2");
        assert_eq!(js_number(&serde_json::Number::from_f64(1.5).unwrap()), "1.5");
        assert_eq!(messages_body(&json!({})), 0);
    }
}
