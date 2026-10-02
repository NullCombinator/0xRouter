//! Media codecs: how `{media}` is written and read in each style.

use nullrouter_registry::schema::MediaCodec;
use serde_json::{Value, json};

use crate::ir::{Media, MediaSource};

/// `{media}` for `m` in `codec`, or `None` when the codec can't carry the source.
pub fn encode(codec: MediaCodec, m: &Media) -> Option<Value> {
    let mime = m.mime.as_deref().unwrap_or("application/octet-stream");
    Some(match (codec, &m.source) {
        (MediaCodec::DataUrl, MediaSource::Base64(d)) => Value::String(format!("data:{mime};base64,{d}")),
        (MediaCodec::DataUrl | MediaCodec::Url, MediaSource::Url(u)) => Value::String(u.clone()),
        (MediaCodec::Url, MediaSource::Base64(_)) => return None,
        (MediaCodec::AnthropicSource, MediaSource::Base64(d)) => {
            json!({ "type": "base64", "media_type": mime, "data": d })
        }
        (MediaCodec::AnthropicSource, MediaSource::Url(u)) => json!({ "type": "url", "url": u }),
        (MediaCodec::GeminiInlineData, MediaSource::Base64(d)) => {
            json!({ "inlineData": { "mimeType": mime, "data": d } })
        }
        (MediaCodec::GeminiInlineData, MediaSource::Url(u)) => match &m.mime {
            Some(mime) => json!({ "fileData": { "mimeType": mime, "fileUri": u } }),
            None => json!({ "fileData": { "fileUri": u } }),
        },
    })
}

/// Reads `{media}` written by `codec`.
pub fn decode(codec: MediaCodec, v: &Value) -> Option<Media> {
    match codec {
        MediaCodec::DataUrl | MediaCodec::Url => {
            let s = v.as_str()?;
            match s.strip_prefix("data:").and_then(|r| r.split_once(";base64,")) {
                Some((mime, data)) if codec == MediaCodec::DataUrl => {
                    Some(Media { mime: Some(mime.to_owned()), source: MediaSource::Base64(data.to_owned()) })
                }
                Some(_) => None,
                None => Some(Media { mime: None, source: MediaSource::Url(s.to_owned()) }),
            }
        }
        MediaCodec::AnthropicSource => match v.get("type")?.as_str()? {
            "base64" => Some(Media {
                mime: v.get("media_type").and_then(Value::as_str).map(str::to_owned),
                source: MediaSource::Base64(v.get("data")?.as_str()?.to_owned()),
            }),
            "url" => Some(Media { mime: None, source: MediaSource::Url(v.get("url")?.as_str()?.to_owned()) }),
            _ => None,
        },
        MediaCodec::GeminiInlineData => {
            let (inner, key, url) = match (v.get("inlineData"), v.get("fileData")) {
                (Some(i), _) => (i, "data", false),
                (None, Some(f)) => (f, "fileUri", true),
                _ => return None,
            };
            let mime = inner.get("mimeType").and_then(Value::as_str).map(str::to_owned);
            let data = inner.get(key)?.as_str()?.to_owned();
            Some(Media { mime, source: if url { MediaSource::Url(data) } else { MediaSource::Base64(data) } })
        }
    }
}

/// The split bindings: `media.mime`, `media.data`, `media.url`.
pub fn fields(m: &Media) -> [(&'static str, Option<Value>); 3] {
    let (data, url) = match &m.source {
        MediaSource::Base64(d) => (Some(Value::String(d.clone())), None),
        MediaSource::Url(u) => (None, Some(Value::String(u.clone()))),
    };
    [("media.mime", m.mime.clone().map(Value::String)), ("media.data", data), ("media.url", url)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codec_round_trips_what_it_carries() {
        let b64 = Media { mime: Some("image/png".into()), source: MediaSource::Base64("AAA".into()) };
        let url = Media { mime: None, source: MediaSource::Url("https://x/y.png".into()) };
        for codec in [MediaCodec::DataUrl, MediaCodec::AnthropicSource, MediaCodec::GeminiInlineData, MediaCodec::Url] {
            for m in [&b64, &url] {
                let Some(v) = encode(codec, m) else {
                    assert_eq!((codec, &m.source), (MediaCodec::Url, &b64.source));
                    continue;
                };
                assert_eq!(decode(codec, &v).as_ref(), Some(m), "{codec:?} {v}");
            }
        }
    }
}
