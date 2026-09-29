//! Body encodings: `json`, `multipart` and `binary`.
//!
//! Templates work on JSON values, so a file travels through them as a file value:
//! `{"$file": {"data": <base64>, "filename": ..., "content_type": ...}}`. A multipart body
//! decodes into an object of text fields and file values, and encodes back from one.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use zerorouter_registry::schema::BodyEncoding;

const FILE: &str = "$file";

/// A file value holding `data`.
pub fn file(data: &[u8], filename: Option<&str>, content_type: Option<&str>) -> Value {
    let mut f = Map::new();
    f.insert("data".into(), Value::String(STANDARD.encode(data)));
    if let Some(n) = filename {
        f.insert("filename".into(), n.into());
    }
    if let Some(c) = content_type {
        f.insert("content_type".into(), c.into());
    }
    json!({ FILE: f })
}

/// A file value's bytes, file name and content type.
pub fn file_parts(v: &Value) -> Option<(Vec<u8>, Option<&str>, Option<&str>)> {
    let f = v.get(FILE)?;
    let data = STANDARD.decode(f.get("data")?.as_str()?).ok()?;
    Some((data, f.get("filename").and_then(Value::as_str), f.get("content_type").and_then(Value::as_str)))
}

/// Bytes from a file value, or from a base64 string (audio in the IR).
pub fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::String(s) => STANDARD.decode(s).ok(),
        other => file_parts(other).map(|(d, ..)| d),
    }
}

/// The multipart boundary from a `content-type` header.
pub fn boundary(content_type: &str) -> Option<&str> {
    content_type.split(';').map(str::trim).find_map(|p| p.strip_prefix("boundary=")).map(|b| b.trim_matches('"'))
}

/// Parses a `multipart/form-data` body. Repeated names become arrays.
pub fn parse_multipart(body: &[u8], boundary: &str) -> Result<Value, String> {
    let delim = format!("--{boundary}");
    let mut out = Map::new();
    let mut rest = body;
    let start = find(rest, delim.as_bytes()).ok_or("no opening boundary")?;
    rest = &rest[start + delim.len()..];
    loop {
        if rest.starts_with(b"--") {
            break;
        }
        rest = rest.strip_prefix(b"\r\n").ok_or("malformed boundary line")?;
        let head_end = find(rest, b"\r\n\r\n").ok_or("a part without headers")?;
        let head = std::str::from_utf8(&rest[..head_end]).map_err(|_| "part headers aren't UTF-8")?;
        rest = &rest[head_end + 4..];
        let end = find(rest, format!("\r\n{delim}").as_bytes()).ok_or("an unterminated part")?;
        let data = &rest[..end];
        rest = &rest[end + 2 + delim.len()..];

        let (mut name, mut filename, mut content_type) = (None, None, None);
        for line in head.split("\r\n") {
            let Some((k, v)) = line.split_once(':') else { continue };
            match k.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => {
                    for p in v.split(';').map(str::trim) {
                        if let Some(n) = p.strip_prefix("name=") {
                            name = Some(n.trim_matches('"').to_owned());
                        } else if let Some(n) = p.strip_prefix("filename=") {
                            filename = Some(n.trim_matches('"').to_owned());
                        }
                    }
                }
                "content-type" => content_type = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        let name = name.ok_or("a part without a name")?;
        let value = match (&filename, std::str::from_utf8(data)) {
            (None, Ok(s)) => Value::String(s.to_owned()),
            _ => file(data, filename.as_deref(), content_type.as_deref()),
        };
        match out.get_mut(&name) {
            Some(Value::Array(a)) => a.push(value),
            Some(prev) => *prev = Value::Array(vec![prev.take(), value]),
            None => {
                out.insert(name, value);
            }
        }
    }
    Ok(Value::Object(out))
}

/// Encodes an object of fields as `multipart/form-data`; returns the body and its
/// content type.
pub fn encode_multipart(fields: &Value) -> (Vec<u8>, String) {
    let mut parts: Vec<(String, Vec<u8>, Option<String>, Option<String>)> = Vec::new();
    let mut push = |name: &str, v: &Value| match v {
        Value::Null => {}
        Value::String(s) => parts.push((name.to_owned(), s.clone().into_bytes(), None, None)),
        v if v.get(FILE).is_some() => {
            if let Some((d, f, c)) = file_parts(v) {
                parts.push((name.to_owned(), d, Some(f.unwrap_or("file").to_owned()), c.map(str::to_owned)));
            }
        }
        other => parts.push((name.to_owned(), other.to_string().into_bytes(), None, None)),
    };
    if let Some(obj) = fields.as_object() {
        for (k, v) in obj {
            match v {
                Value::Array(a) => a.iter().for_each(|el| push(k, el)),
                v => push(k, v),
            }
        }
    }
    let mut h = Sha256::new();
    for (n, d, ..) in &parts {
        h.update(n.as_bytes());
        h.update(d);
    }
    let boundary = format!("0router-{:x}", h.finalize())[..40].to_owned();
    let mut body = Vec::new();
    for (name, data, filename, ctype) in parts {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes());
        if let Some(f) = filename {
            body.extend_from_slice(format!("; filename=\"{f}\"").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        if let Some(c) = ctype {
            body.extend_from_slice(format!("Content-Type: {c}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={boundary}"))
}

/// Encodes a rendered body; returns the bytes and their content type.
pub fn encode(encoding: BodyEncoding, v: &Value) -> (Vec<u8>, String) {
    match encoding {
        BodyEncoding::Json => (v.to_string().into_bytes(), "application/json".into()),
        BodyEncoding::Multipart => encode_multipart(v),
        BodyEncoding::Binary => {
            let ctype = file_parts(v).and_then(|(_, _, c)| c.map(str::to_owned));
            (bytes_of(v).unwrap_or_default(), ctype.unwrap_or_else(|| "application/octet-stream".into()))
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_round_trips_text_and_files() {
        let v = json!({"model_id": "scribe_v2", "file": file(b"RIFF\0\x01", Some("a.wav"), Some("audio/wav")), "tag": ["a", "b"]});
        let (body, ctype) = encode(BodyEncoding::Multipart, &v);
        let back = parse_multipart(&body, boundary(&ctype).unwrap()).unwrap();
        assert_eq!(back, v);
        let (data, name, c) = file_parts(&back["file"]).unwrap();
        assert_eq!((data.as_slice(), name, c), (&b"RIFF\0\x01"[..], Some("a.wav"), Some("audio/wav")));
    }

    #[test]
    fn a_curl_style_body_parses() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper-1\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.mp3\"\r\nContent-Type: audio/mpeg\r\n\r\nID3\xff\r\n--XyZ--\r\n";
        let v = parse_multipart(body, boundary("multipart/form-data; boundary=\"XyZ\"").unwrap()).unwrap();
        assert_eq!(v["model"], "whisper-1");
        assert_eq!(bytes_of(&v["file"]).unwrap(), b"ID3\xff");
        assert!(parse_multipart(b"--XyZ\r\nnope", "XyZ").is_err());
    }

    #[test]
    fn binary_takes_the_file_bytes() {
        let (b, c) = encode(BodyEncoding::Binary, &file(b"abc", None, Some("audio/wav")));
        assert_eq!((b.as_slice(), c.as_str()), (&b"abc"[..], "audio/wav"));
        assert_eq!(encode(BodyEncoding::Binary, &Value::String(STANDARD.encode("xy"))).0, b"xy");
    }
}
