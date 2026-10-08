//! hermes, the built-in adapter (research R4). hermes talks openai-chat; this adapter fixes the
//! two things its requests carry that a provider may not take:
//!
//! - echoed reasoning on assistant messages, removed only for providers in
//!   [`REJECTS_ECHOED_REASONING`] and only on same-style attempts (across styles slice 003's
//!   encoder already records what it drops);
//! - Ollama-style `images` and `attachments`, converted to openai-chat content parts, which
//!   the codec then carries to any target style.
//!
//! It never touches `tools`, `tool_calls` or a `role:"tool"` message, and it never deletes
//! content: what it cannot convert stays where it was.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use nullrouter_adapter_kit::{Context, Edits, Path, Reason};
use serde_json::{Value, json};

use crate::selector::Selector;

pub const NAME: &str = "hermes";

/// The assistant-message fields hermes echoes back from a reasoning model.
const ECHOED: &[&str] = &["reasoning_content", "reasoning", "reasoning_details"];

/// Providers that answer 400 or 422 when an assistant message carries an echoed reasoning
/// field, and the fields they reject. The default for a provider not listed is to keep it
/// (openrouter continues reasoning across tool calls with `reasoning_details`).
///
/// Seeded from 9router's `paramSupport.js` (groq, mistral, cerebras). The chosen providers are
/// added by the live check (T031) and only if they returned a 400 naming the field.
pub const REJECTS_ECHOED_REASONING: &[(&str, &[&str])] =
    &[("groq", ECHOED), ("mistral", ECHOED), ("cerebras", ECHOED)];

const IMAGE_FIELD: &str = "images";
const ATTACHMENT_FIELDS: [&str; 2] = ["attachments", "experimental_attachments"];

/// What hermes reads. `content` and `role` are read to build the converted message.
pub fn request_selectors() -> Vec<Selector> {
    [
        "messages[*].reasoning_content",
        "messages[*].reasoning",
        "messages[*].reasoning_details",
        "messages[*].images",
        "messages[*].attachments",
        "messages[*].experimental_attachments",
        "messages[*].content",
        "messages[*].role",
    ]
    .iter()
    .map(|s| Selector::parse(s).expect("a valid selector"))
    .collect()
}

fn rejected_fields(provider: &str) -> &'static [&'static str] {
    REJECTS_ECHOED_REASONING.iter().find(|(p, _)| *p == provider).map_or(&[], |(_, f)| *f)
}

pub fn on_request(ctx: &Context, body: &Value) -> Edits {
    let mut out = Edits::default();
    let Some(messages) = body.get("messages").and_then(Value::as_array) else { return out };
    let at = Path::root().child("messages");
    for (i, m) in messages.iter().enumerate() {
        let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
        let here = at.index(i);
        if ctx.same_style && role == "assistant" {
            for f in rejected_fields(&ctx.provider) {
                if m.get(*f).is_some() {
                    out.remove(&here.child(f), Reason::TargetRejectsField);
                }
            }
        }
        if role == "user" {
            convert_media(m, &here, &mut out);
        }
    }
    out
}

/// Moves a user message's `images` and attachments into its `content` as content parts.
///
/// Entries that can't be converted (no MIME type to be had, or one no model reads) stay in
/// their field, so slice 003 sees them and decides about the target. A message with no
/// `content` key is left alone, since an edit can only replace a key that exists.
fn convert_media(m: &Value, here: &Path, out: &mut Edits) {
    let Some(content) = m.get("content") else { return };
    let mut parts = Vec::new();
    // (field, what stays in it) for every field that had something converted.
    let mut fields: Vec<(&str, Vec<Value>)> = Vec::new();

    if let Some(list) = m.get(IMAGE_FIELD).and_then(Value::as_array) {
        let (done, left) = split(list, image_part);
        if !done.is_empty() {
            parts.extend(done);
            fields.push((IMAGE_FIELD, left));
        }
    }
    for f in ATTACHMENT_FIELDS {
        if let Some(list) = m.get(f).and_then(Value::as_array) {
            let (done, left) = split(list, attachment_part);
            if !done.is_empty() {
                parts.extend(done);
                fields.push((f, left));
            }
        }
    }
    if parts.is_empty() {
        return;
    }

    let mut merged = match content {
        Value::String(s) if !s.is_empty() => vec![json!({"type": "text", "text": s})],
        Value::Array(a) => a.clone(),
        _ => Vec::new(),
    };
    merged.extend(parts);
    out.convert(&here.child("content"), Value::Array(merged), Reason::FormatConversion);
    for (f, left) in fields {
        if left.is_empty() {
            out.remove(&here.child(f), Reason::FormatConversion);
        } else {
            out.convert(&here.child(f), Value::Array(left), Reason::FormatConversion);
        }
    }
}

/// `list` split into the parts `make` could build, and the entries it could not.
fn split(list: &[Value], make: fn(&Value) -> Option<Value>) -> (Vec<Value>, Vec<Value>) {
    let (mut done, mut left) = (Vec::new(), Vec::new());
    for v in list {
        match make(v) {
            Some(p) => done.push(p),
            None => left.push(v.clone()),
        }
    }
    (done, left)
}

fn image_url(url: &str) -> Value {
    json!({"type": "image_url", "image_url": {"url": url}})
}

/// An entry of `images`: a raw base64 string, `{data, mime}`, or `{url}`.
fn image_part(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) => data_url(s, None).map(|u| image_url(&u)),
        Value::Object(o) => {
            let mime = text(o.get("mime").or_else(|| o.get("mime_type")).or_else(|| o.get("contentType")));
            if let Some(d) = o.get("data").and_then(Value::as_str) {
                return data_url(d, mime).map(|u| image_url(&u));
            }
            let url = o.get("url").and_then(Value::as_str)?;
            (url.starts_with("data:image/") || url.starts_with("http://") || url.starts_with("https://"))
                .then(|| image_url(url))
        }
        _ => None,
    }
}

/// An entry of `attachments` or `experimental_attachments`: `{url | data, contentType |
/// mediaType, name}`. Images become `image_url` parts, PDFs `file` parts.
fn attachment_part(v: &Value) -> Option<Value> {
    let o = v.as_object()?;
    let url = o.get("url").and_then(Value::as_str);
    let data = o.get("data").and_then(Value::as_str);
    let mime = text(o.get("contentType").or_else(|| o.get("mediaType")).or_else(|| o.get("mime")))
        .map(str::to_owned)
        .or_else(|| url.and_then(url_mime).map(str::to_owned))?;
    if mime.starts_with("image/") {
        let link = match (data, url) {
            (Some(d), _) => data_url(d, Some(&mime))?,
            (None, Some(u)) if u.starts_with("data:") || u.starts_with("http") => u.to_owned(),
            _ => return None,
        };
        return Some(image_url(&link));
    }
    if mime == "application/pdf" {
        let file_data = match (data, url) {
            (Some(d), _) => format!("data:{mime};base64,{}", strip_prefix(d)),
            (None, Some(u)) if u.starts_with("data:") => u.to_owned(),
            _ => return None,
        };
        let filename = o.get("name").or_else(|| o.get("filename")).and_then(Value::as_str).unwrap_or("document.pdf");
        return Some(json!({"type": "file", "file": {"file_data": file_data, "filename": filename}}));
    }
    None
}

fn text(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The MIME type of a `data:<mime>;base64,` URL.
fn url_mime(url: &str) -> Option<&str> {
    url.strip_prefix("data:")?.split([';', ',']).next().filter(|m| !m.is_empty())
}

/// Base64 without a `data:...;base64,` prefix.
fn strip_prefix(s: &str) -> &str {
    s.strip_prefix("data:").and_then(|r| r.split_once(',')).map_or(s, |(_, d)| d)
}

/// A `data:` URL for base64 `data`. `mime` is the declared type; without one the type is
/// sniffed from the bytes (PNG, JPEG, GIF, WebP). `None` when neither gives an image type.
fn data_url(data: &str, mime: Option<&str>) -> Option<String> {
    if data.starts_with("data:image/") {
        return Some(data.to_owned());
    }
    let b64 = strip_prefix(data);
    let mime = match mime {
        Some(m) if m.starts_with("image/") => m.to_owned(),
        Some(_) => return None,
        None => sniff(b64)?.to_owned(),
    };
    Some(format!("data:{mime};base64,{b64}"))
}

/// The image type of base64 `b64`, from its first bytes.
fn sniff(b64: &str) -> Option<&'static str> {
    // 16 base64 characters decode to 12 bytes, enough for every signature below.
    let head = b64.get(..b64.len().min(16) / 4 * 4)?;
    let bytes = STANDARD.decode(head).ok()?;
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
