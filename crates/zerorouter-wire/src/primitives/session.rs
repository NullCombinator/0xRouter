//! Client session carriers and provider session derivations (contracts/api-style-schema.md
//! § Session; contracts/provider-schema-v2.md § Session). 9router oracle
//! `open-sse/utils/sessionManager.js` and `open-sse/executors/opencode.js`.

use serde_json::Value;
use sha2::{Digest, Sha256};
use zerorouter_registry::schema::{SessionCarrier, SessionDerive, SessionExtractor};
use zerorouter_registry::template::FieldPath;

use crate::template::select_one;

/// 9router `normalizeSessionId`: longer values are refused, not cut.
pub const MAX_CHARS: usize = 256;

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn normalize(v: &str) -> Option<String> {
    let v = v.trim();
    (!v.is_empty() && v.chars().count() <= MAX_CHARS).then(|| v.to_owned())
}

/// The client's session id from the first carrier that holds a usable one, in the style's
/// order. `header` looks a header up by name, case-insensitively.
pub fn extract<'h>(carriers: &[SessionCarrier], header: impl Fn(&str) -> Option<&'h str>, body: &Value) -> Option<String> {
    carriers.iter().find_map(|c| {
        if let Some(h) = &c.header {
            normalize(header(h)?)
        } else if let Some(p) = &c.path {
            normalize(select_one(&FieldPath::parse(p).ok()?, body)?.as_str()?)
        } else {
            match c.extractor? {
                SessionExtractor::ClaudeCodeUserId => claude_code_user_id(body.pointer("/metadata/user_id")?.as_str()?),
            }
        }
    })
}

/// Claude Code's `metadata.user_id`: `…_session_{uuid}`, or JSON with a `session_id`.
fn claude_code_user_id(user_id: &str) -> Option<String> {
    if let Some((_, tail)) = user_id.rsplit_once("_session_")
        && !tail.is_empty()
        && tail.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-'))
    {
        return Some(tail.to_owned());
    }
    if user_id.starts_with('{') {
        let v: Value = serde_json::from_str(user_id).ok()?;
        return normalize(v.get("session_id")?.as_str()?);
    }
    None
}

/// `ses_` + 12 lowercase hex + 14 base62: opencode's canonical session shape.
pub fn is_opencode_session(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 30
        && s.starts_with("ses_")
        && b[4..16].iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        && b[16..].iter().all(u8::is_ascii_alphanumeric)
}

/// The provider session header value for an attempt. `input` is the agent id; `client` is
/// the value the client sent in the same header, if any, which wins when the provider
/// would accept it (9router keeps a native `x-opencode-session`).
pub fn derive(d: SessionDerive, input: &str, client: Option<&str>) -> String {
    let client = client.and_then(normalize);
    match d {
        SessionDerive::SesSha256Hex32 => client.unwrap_or_else(|| ses_sha256_hex32(input)),
        SessionDerive::SesTimeBase62 => client.filter(|c| is_opencode_session(c)).unwrap_or_else(|| ses_time_base62(input)),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 9router opencode-go `translatedSession(input)`: `ses_` + 32 hex of a salted SHA-256.
pub fn ses_sha256_hex32(input: &str) -> String {
    let digest = Sha256::digest(format!("opencode-go\0generic\0{input}").as_bytes());
    format!("ses_{}", hex(&digest[..16]))
}

/// 9router opencode-zen `translateSessionId(input)`: the canonical shape, with the time
/// slot and the base62 tail taken from a salted SHA-256. An input already in that shape
/// passes through.
pub fn ses_time_base62(input: &str) -> String {
    let trimmed = input.trim();
    if is_opencode_session(trimmed) {
        return trimmed.to_owned();
    }
    let digest = Sha256::digest(format!("opencode\0generic\0{input}").as_bytes());
    let mut out = format!("ses_{}", hex(&digest[..6]));
    out.extend(digest[6..20].iter().map(|b| BASE62[usize::from(*b) % 62] as char));
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn carrier(header: Option<&str>, path: Option<&str>, extractor: Option<SessionExtractor>) -> SessionCarrier {
        SessionCarrier { header: header.map(Into::into), path: path.map(Into::into), extractor }
    }

    #[test]
    fn claude_code_user_ids_carry_the_session() {
        let id = "user_ab12_account__session_550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(claude_code_user_id(id).as_deref(), Some("550e8400-e29b-41d4-a716-446655440000"));
        assert_eq!(claude_code_user_id(r#"{"device_id":"d","session_id":" s-1 "}"#).as_deref(), Some("s-1"));
        assert_eq!(claude_code_user_id("user_ab12"), None);
        assert_eq!(claude_code_user_id("x_session_NOTHEX"), None);
        assert_eq!(claude_code_user_id("{not json"), None);
    }

    #[test]
    fn carriers_are_read_in_order() {
        let carriers = [
            carrier(None, None, Some(SessionExtractor::ClaudeCodeUserId)),
            carrier(Some("x-claude-code-session-id"), None, None),
            carrier(None, Some("prompt_cache_key"), None),
        ];
        let headers = |h: &str| (h == "x-claude-code-session-id").then_some(" h-1 ");
        let none = |_: &str| None;
        let body = json!({ "metadata": { "user_id": "u_session_abc-1" }, "prompt_cache_key": "p-1" });
        assert_eq!(extract(&carriers, headers, &body).as_deref(), Some("abc-1"));
        assert_eq!(extract(&carriers, headers, &json!({ "prompt_cache_key": "p-1" })).as_deref(), Some("h-1"));
        assert_eq!(extract(&carriers, none, &json!({ "prompt_cache_key": "p-1" })).as_deref(), Some("p-1"));
        assert_eq!(extract(&carriers, none, &json!({ "prompt_cache_key": "  " })), None);
        let long = "x".repeat(MAX_CHARS + 1);
        assert_eq!(extract(&carriers, none, &json!({ "prompt_cache_key": long })), None, "too long is refused");
    }

    /// Values from 9router's opencode-go and opencode-zen executors at the ref SHA
    /// (`prepareRequestCredentials` with `providerSessionId`).
    #[test]
    fn derivations_match_9router() {
        assert_eq!(ses_sha256_hex32("ak_1"), "ses_48d2096cf76890edfdd9ded07b2feab2");
        assert_eq!(ses_sha256_hex32("ak_1:sess-9"), "ses_1ce65a94e29da7cfa2470cf567489c1f");
        assert_eq!(ses_time_base62("ak_1"), "ses_44e3e1800ab62TfohCicJxGYo3");
        assert_eq!(ses_time_base62("ak_1:sess-9"), "ses_42acf87b82d3cesOIUYHR5JmrR");
        assert_eq!(ses_time_base62(""), "ses_a7f9702bdb4d5CgHPQUAk1uIqH");
        let valid = "ses_f534dfae8ffeCy4Ee4tLWNygDc";
        assert_eq!(ses_time_base62(&format!("  {valid}  ")), valid);
        assert!(is_opencode_session(&ses_time_base62("x")));
    }

    #[test]
    fn a_native_client_session_wins_where_the_provider_takes_it() {
        let valid = "ses_f534dfae8ffeCy4Ee4tLWNygDc";
        assert_eq!(derive(SessionDerive::SesSha256Hex32, "ak_1", Some(" whatever-1 ")), "whatever-1");
        assert_eq!(derive(SessionDerive::SesSha256Hex32, "ak_1", Some("  ")), ses_sha256_hex32("ak_1"));
        assert_eq!(derive(SessionDerive::SesTimeBase62, "ak_1", Some(valid)), valid);
        assert_eq!(derive(SessionDerive::SesTimeBase62, "ak_1", Some("invalid-uuid")), ses_time_base62("ak_1"));
        assert_eq!(derive(SessionDerive::SesTimeBase62, "ak_1", None), ses_time_base62("ak_1"));
    }
}
