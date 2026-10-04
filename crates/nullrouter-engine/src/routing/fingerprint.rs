//! Prompt-prefix fingerprints (research R3).
//!
//! A request's prefix chain is `h0 = H(salt, model, tools, system)`, then `hk = H(h(k-1),
//! message k)`. Each hash is one boundary, with the prefix length in estimated tokens. Only
//! hashes leave this module; no prompt text is kept. `cache_control` values are not hashed: they
//! don't change what the provider caches as content.

use std::fmt;
use std::time::Duration;

use nullrouter_registry::schema::CacheMode;
use nullrouter_wire::ir::{MediaSource, Part, Request, ResultContent, Role, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::router::Salt;

/// A SHA-256 truncated to 128 bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash(pub [u8; 16]);

impl Hash {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 32 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.hex())
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl Serialize for Hash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Hash::from_hex(&text).ok_or_else(|| serde::de::Error::custom("not a 128-bit hex hash"))
    }
}

/// One point in a request's prefix where a provider cache could end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary {
    pub hash: Hash,
    /// Estimated tokens from the start of the request up to this boundary.
    pub prefix_tokens: u64,
    /// The lifetime a cache marker at or after this boundary asked for (`ttl`), if any.
    pub marker_ttl: Option<Duration>,
    /// A marker sits at or after this boundary, so an explicit cache keeps it.
    pub covered: bool,
}

/// The prefix chain of one request: `boundaries[0]` is tools and system, then one per message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub boundaries: Vec<Boundary>,
}

/// A fingerprint to remember after a request succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    pub hash: Hash,
    pub prefix_tokens: u64,
    pub ttl: Option<Duration>,
}

impl Chain {
    /// `ttl_key` is where the client style declares a marker's lifetime inside `cache_control`.
    pub fn build(req: &Request, salt: &Salt, ttl_key: Option<&str>) -> Self {
        // A system message inside the conversation's head counts as system, so a style that
        // carries the system prompt as a message gives the same chain as one with a field.
        let lead = req.messages.iter().take_while(|m| m.role == Role::System).count();
        let (head, rest) = req.messages.split_at(lead);

        let mut sizes = Vec::with_capacity(rest.len() + 1);
        let mut marks: Vec<Option<Option<Duration>>> = Vec::with_capacity(rest.len() + 1);
        let mut hashes = Vec::with_capacity(rest.len() + 1);

        let mut h = Sha256::new();
        h.update(salt.bytes());
        let mut bytes = 0u64;
        let mut mark = None;
        text(&mut h, &mut bytes, TAG_MODEL, &req.model);
        for tool in &req.tools {
            self::tool(&mut h, &mut bytes, tool);
            note_marker(&mut mark, tool.cache_control.as_ref(), ttl_key);
        }
        for part in req.system.iter().chain(head.iter().flat_map(|m| m.parts.iter())) {
            self::part(&mut h, &mut bytes, part);
            note_marker(&mut mark, part.cache_control(), ttl_key);
        }
        let mut prev = finish(h);
        hashes.push(prev);
        sizes.push(bytes);
        marks.push(mark);

        for msg in rest {
            let mut h = Sha256::new();
            h.update(prev.0);
            let mut mark = None;
            h.update([TAG_ROLE, msg.role as u8]);
            for p in &msg.parts {
                self::part(&mut h, &mut bytes, p);
                note_marker(&mut mark, p.cache_control(), ttl_key);
            }
            prev = finish(h);
            hashes.push(prev);
            sizes.push(bytes);
            marks.push(mark);
        }

        // A marker covers every boundary up to itself; its ttl belongs to the boundaries it covers
        // until the next marker above them.
        let mut boundaries: Vec<Boundary> = Vec::with_capacity(hashes.len());
        let mut next: Option<Option<Duration>> = None;
        for i in (0..hashes.len()).rev() {
            if let Some(m) = marks[i] {
                next = Some(m);
            }
            boundaries.push(Boundary {
                hash: hashes[i],
                prefix_tokens: sizes[i].div_ceil(4),
                marker_ttl: next.flatten(),
                covered: next.is_some(),
            });
        }
        boundaries.reverse();
        Self { boundaries }
    }

    /// The boundaries a provider with this cache holds after the request succeeded.
    pub fn writable(&self, mode: CacheMode, min_tokens: u64) -> Vec<Written> {
        let keep = |b: &&Boundary| match mode {
            CacheMode::Explicit => b.covered,
            CacheMode::Automatic => b.prefix_tokens >= min_tokens,
            CacheMode::Disabled => false,
        };
        self.boundaries
            .iter()
            .filter(keep)
            .map(|b| Written { hash: b.hash, prefix_tokens: b.prefix_tokens, ttl: b.marker_ttl })
            .collect()
    }
}

fn note_marker(slot: &mut Option<Option<Duration>>, cache_control: Option<&Value>, ttl_key: Option<&str>) {
    let Some(cc) = cache_control else { return };
    let ttl = ttl_key
        .and_then(|k| cc.get(k))
        .and_then(Value::as_str)
        .and_then(|t| nullrouter_registry::schema::parse_duration(t).ok());
    // The longest lifetime asked for among the markers of one message wins.
    *slot = Some(match (*slot, ttl) {
        (Some(Some(a)), Some(b)) => Some(a.max(b)),
        (Some(a), None) => a,
        (_, b) => b,
    });
}

const TAG_MODEL: u8 = 1;
const TAG_TOOL: u8 = 2;
const TAG_ROLE: u8 = 3;
const TAG_TEXT: u8 = 4;
const TAG_IMAGE: u8 = 5;
const TAG_AUDIO: u8 = 6;
const TAG_CALL: u8 = 7;
const TAG_RESULT: u8 = 8;
const TAG_THINKING: u8 = 9;
const TAG_JSON: u8 = 10;
const TAG_URL: u8 = 11;

fn finish(h: Sha256) -> Hash {
    let full = h.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&full[..16]);
    Hash(out)
}

/// A tagged, length-prefixed field: neighbours can't run into each other.
fn text(h: &mut Sha256, bytes: &mut u64, tag: u8, s: &str) {
    h.update([tag]);
    h.update((s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
    if tag != TAG_MODEL {
        *bytes += s.len() as u64;
    }
}

fn json(h: &mut Sha256, bytes: &mut u64, v: &Value) {
    let mut canonical = String::new();
    canon(v, &mut canonical);
    text(h, bytes, TAG_JSON, &canonical);
}

/// JSON with object keys sorted, so key order in a client's body never changes a hash.
fn canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canon(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canon(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn tool(h: &mut Sha256, bytes: &mut u64, t: &Tool) {
    text(h, bytes, TAG_TOOL, &t.name);
    text(h, bytes, TAG_TEXT, t.description.as_deref().unwrap_or(""));
    json(h, bytes, &t.parameters);
}

fn media(h: &mut Sha256, bytes: &mut u64, tag: u8, m: &nullrouter_wire::ir::Media) {
    text(h, bytes, tag, m.mime.as_deref().unwrap_or(""));
    match &m.source {
        MediaSource::Base64(b) => text(h, bytes, TAG_JSON, b),
        MediaSource::Url(u) => text(h, bytes, TAG_URL, u),
    }
}

fn part(h: &mut Sha256, bytes: &mut u64, p: &Part) {
    match p {
        Part::Text { text: t, .. } => text(h, bytes, TAG_TEXT, t),
        Part::Image { media: m, .. } => media(h, bytes, TAG_IMAGE, m),
        Part::Audio { media: m, .. } => media(h, bytes, TAG_AUDIO, m),
        Part::ToolCall { id, name, arguments, .. } => {
            text(h, bytes, TAG_CALL, id);
            text(h, bytes, TAG_TEXT, name);
            json(h, bytes, arguments);
        }
        Part::ToolResult { id, name, content, is_error, .. } => {
            text(h, bytes, TAG_RESULT, id);
            text(h, bytes, TAG_TEXT, name.as_deref().unwrap_or(""));
            h.update([u8::from(*is_error)]);
            match content {
                ResultContent::Text(t) => text(h, bytes, TAG_TEXT, t),
                ResultContent::Parts(ps) => ps.iter().for_each(|p| part(h, bytes, p)),
                ResultContent::Json(v) => json(h, bytes, v),
            }
        }
        // The signature is the provider's, not the conversation's.
        Part::Thinking { text: t, .. } => text(h, bytes, TAG_THINKING, t),
    }
}

#[cfg(test)]
mod tests {
    use nullrouter_registry::validate::validate_style;
    use nullrouter_wire::codec::{Style, request};
    use serde_json::json;

    use super::*;

    fn style(id: &str) -> Style {
        let (path, src) = match id {
            "anthropic-messages" => ("anthropic-messages", include_str!("../../../../styles/bundled/anthropic-messages.toml")),
            "openai-chat" => ("openai-chat", include_str!("../../../../styles/bundled/openai-chat.toml")),
            other => panic!("no style {other}"),
        };
        Style::compile(&validate_style(src, path).unwrap()).unwrap()
    }

    fn salt(n: u8) -> Salt {
        Salt::from_bytes([n; 32])
    }

    fn chain_of(body: Value, style_id: &str, salt: &Salt) -> Chain {
        let ir = request::decode(&style(style_id), &body).unwrap();
        Chain::build(&ir, salt, Some("ttl"))
    }

    fn messages_body(model: &str, marker: Option<Value>) -> Value {
        let mut system = json!({"type": "text", "text": "You are terse."});
        if let Some(m) = &marker {
            system["cache_control"] = m.clone();
        }
        json!({
            "model": model, "max_tokens": 10,
            "system": [system],
            "messages": [
                {"role": "user", "content": "What is two plus two?"},
                {"role": "assistant", "content": "Four."},
                {"role": "user", "content": "And three plus three?"},
            ],
        })
    }

    fn chat_body(model: &str) -> Value {
        json!({
            "model": model,
            "messages": [
                {"role": "system", "content": "You are terse."},
                {"role": "user", "content": "What is two plus two?"},
                {"role": "assistant", "content": "Four."},
                {"role": "user", "content": "And three plus three?"},
            ],
        })
    }

    #[test]
    fn one_conversation_gives_one_chain_in_either_style() {
        let s = salt(1);
        let a = chain_of(messages_body("m", None), "anthropic-messages", &s);
        let b = chain_of(chat_body("m"), "openai-chat", &s);
        let hashes = |c: &Chain| c.boundaries.iter().map(|b| b.hash).collect::<Vec<_>>();
        assert_eq!(a.boundaries.len(), 4);
        assert_eq!(hashes(&a), hashes(&b));
    }

    #[test]
    fn the_upstream_model_is_part_of_the_first_hash() {
        let s = salt(1);
        let a = chain_of(chat_body("m1"), "openai-chat", &s);
        let b = chain_of(chat_body("m2"), "openai-chat", &s);
        assert!(a.boundaries.iter().zip(&b.boundaries).all(|(x, y)| x.hash != y.hash));
    }

    #[test]
    fn marker_values_do_not_change_hashes_but_the_salt_does() {
        let plain = chain_of(messages_body("m", None), "anthropic-messages", &salt(1));
        let marked = chain_of(messages_body("m", Some(json!({"type": "ephemeral"}))), "anthropic-messages", &salt(1));
        let hour = chain_of(messages_body("m", Some(json!({"type": "ephemeral", "ttl": "1h"}))), "anthropic-messages", &salt(1));
        let other = chain_of(messages_body("m", None), "anthropic-messages", &salt(2));
        let hashes = |c: &Chain| c.boundaries.iter().map(|b| b.hash).collect::<Vec<_>>();
        assert_eq!(hashes(&plain), hashes(&marked));
        assert_eq!(hashes(&plain), hashes(&hour));
        assert!(plain.boundaries.iter().zip(&other.boundaries).all(|(x, y)| x.hash != y.hash));
        assert_eq!(hour.boundaries[0].marker_ttl, Some(Duration::from_secs(3600)));
        assert_eq!(marked.boundaries[0].marker_ttl, None);
        // The ttl key comes from the style's data, not from the core.
        assert_eq!(style("anthropic-messages").cache_ttl_key.as_deref(), Some("ttl"));
        assert_eq!(style("openai-chat").cache_ttl_key, None);
    }

    #[test]
    fn a_changed_message_changes_only_the_boundaries_from_it_on() {
        let s = salt(1);
        let a = chain_of(chat_body("m"), "openai-chat", &s);
        let mut body = chat_body("m");
        body["messages"][3]["content"] = json!("And four plus four?");
        let b = chain_of(body, "openai-chat", &s);
        assert_eq!(a.boundaries[..3], b.boundaries[..3]);
        assert_ne!(a.boundaries[3].hash, b.boundaries[3].hash);
    }

    #[test]
    fn explicit_mode_writes_only_what_a_marker_covers() {
        let s = salt(1);
        let unmarked = chain_of(messages_body("m", None), "anthropic-messages", &s);
        assert!(unmarked.writable(CacheMode::Explicit, 1024).is_empty());

        // A marker on the second message: boundaries 0..=2 are covered, not the last.
        let mut body = messages_body("m", None);
        body["messages"][1]["content"] = json!([{"type": "text", "text": "Four.", "cache_control": {"type": "ephemeral"}}]);
        let c = chain_of(body, "anthropic-messages", &s);
        let written = c.writable(CacheMode::Explicit, 1024);
        assert_eq!(written.len(), 3);
        assert_eq!(written[2].hash, c.boundaries[2].hash);
        assert!(written.iter().all(|w| w.hash != c.boundaries[3].hash));

        // A marker on the system block alone covers the system and tools boundary only.
        let c = chain_of(messages_body("m", Some(json!({"type": "ephemeral"}))), "anthropic-messages", &s);
        assert_eq!(c.writable(CacheMode::Explicit, 1024).len(), 1);
    }

    #[test]
    fn automatic_mode_writes_boundaries_at_the_minimum_and_none_writes_nothing() {
        let s = salt(1);
        let mut body = chat_body("m");
        body["messages"][0]["content"] = json!("x".repeat(4096));
        let c = chain_of(body, "openai-chat", &s);
        assert_eq!(c.writable(CacheMode::Automatic, 1024).len(), 4);
        assert!(c.writable(CacheMode::Automatic, 1100).is_empty());
        let some = c.writable(CacheMode::Automatic, 1025).len();
        assert!((1..4).contains(&some), "{some}");
        let small = chain_of(chat_body("m"), "openai-chat", &s);
        assert!(small.writable(CacheMode::Automatic, 1024).is_empty());
        assert_eq!(small.writable(CacheMode::Automatic, 1).len(), 4);
        assert!(c.writable(CacheMode::Disabled, 1).is_empty());
    }

    #[test]
    fn prefix_tokens_grow_along_the_chain() {
        let c = chain_of(chat_body("m"), "openai-chat", &salt(1));
        let t: Vec<u64> = c.boundaries.iter().map(|b| b.prefix_tokens).collect();
        assert!(t.windows(2).all(|w| w[0] < w[1]), "{t:?}");
        // "You are terse." is 14 bytes: 4 tokens.
        assert_eq!(t[0], 4);
    }

    #[test]
    fn output_is_hex_and_carries_no_prompt_text() {
        let secret = "needle-in-the-prompt";
        let mut body = chat_body("m");
        body["messages"][1]["content"] = json!(secret);
        let c = chain_of(body, "openai-chat", &salt(1));
        let written = c.writable(CacheMode::Automatic, 1);
        let rendered = format!("{c:?} {written:?} {}", serde_json::to_string(&written.iter().map(|w| w.hash).collect::<Vec<_>>()).unwrap());
        assert!(!rendered.contains(secret));
        for b in &c.boundaries {
            let hex = b.hash.hex();
            assert_eq!(hex.len(), 32);
            assert!(hex.bytes().all(|x| x.is_ascii_hexdigit()));
            assert_eq!(Hash::from_hex(&hex), Some(b.hash));
        }
    }
}
