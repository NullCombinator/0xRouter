//! The smallest request per type and its PASS check (research R2, after 9router's `ping.js`).
//!
//! Bodies are written in the Chat Completions style and decoded by its codecs, so every wire
//! encodes them as it would a client's. A TTS body names no voice, so the endpoint's first
//! declared voice applies; plugins declare no sizes or durations, so the provider's defaults do.

use nullrouter_registry::schema::ModelType;
use nullrouter_wire::codec::types::TypeValue;
use nullrouter_wire::ir;
use serde_json::{Value, json};

/// The client style a test speaks.
pub const STYLE: &str = "openai-chat";

/// The prompt for text, image and video tests.
pub const PROMPT: &str = "a red dot";

/// 9router's text budget: reasoning models spend a small one on thinking and answer nothing.
pub const MAX_TOKENS: u64 = 1024;

/// The request body for a model of type `ty`.
pub fn body(ty: ModelType) -> Value {
    match ty {
        ModelType::Embeddings => json!({"model": "test", "input": "test"}),
        ModelType::Tts => json!({"model": "test", "input": "test"}),
        ModelType::Stt => json!({
            "model": "test",
            "file": nullrouter_wire::primitives::body::file(&silent_wav(), Some("silence.wav"), Some("audio/wav")),
        }),
        ModelType::Image => json!({"model": "test", "prompt": PROMPT, "n": 1}),
        ModelType::Video => json!({"model": "test", "prompt": PROMPT}),
        ModelType::Text => json!({
            "model": "test",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": MAX_TOKENS,
            "stream": false,
        }),
    }
}

/// 250 ms of 16 kHz mono 16-bit silence as a WAV file.
pub fn silent_wav() -> Vec<u8> {
    const RATE: u32 = 16_000;
    let samples = RATE / 4;
    let data = samples * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.resize(44 + data as usize, 0);
    out
}

/// A text answer passes with at least one content part or a finish reason.
pub fn check_text(r: &ir::Response) -> Result<(), String> {
    if r.content.is_empty() && r.finish.is_none() {
        return Err("malformed answer: no content and no finish reason".into());
    }
    Ok(())
}

/// A decoded non-text answer passes per research R2's table.
pub fn check_value(ty: ModelType, v: &TypeValue) -> Result<(), String> {
    let ok = match ty {
        ModelType::Embeddings => v
            .items
            .iter()
            .filter_map(|i| i.get("output.embedding"))
            .any(|e| e.as_array().is_some_and(|a| a.iter().any(Value::is_number))),
        // Silence may well transcribe to nothing: a `text` field is enough.
        ModelType::Stt => v.get("output.text").is_some_and(|t| t.is_string()),
        ModelType::Image => {
            v.items.iter().any(|i| i.get("output.url").is_some() || i.get("output.b64_json").is_some())
        }
        ModelType::Tts => v.get("output.audio").is_some(),
        _ => !v.items.is_empty() || !v.scalars.0.is_empty(),
    };
    if ok { Ok(()) } else { Err(format!("malformed answer: no {} in the response", what(ty))) }
}

fn what(ty: ModelType) -> &'static str {
    match ty {
        ModelType::Embeddings => "vector",
        ModelType::Stt => "transcript",
        ModelType::Image => "image",
        ModelType::Tts => "audio",
        ModelType::Video => "video",
        _ => "content",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_silent_wav_is_250_ms_at_16_khz() {
        let wav = silent_wav();
        assert_eq!(wav.len(), 44 + 8000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert!(wav[44..].iter().all(|b| *b == 0));
    }

    #[test]
    fn text_needs_content_or_a_finish() {
        assert!(check_text(&ir::Response::default()).is_err());
        let r = ir::Response { finish: Some(ir::FinishReason::Stop), ..Default::default() };
        assert!(check_text(&r).is_ok());
    }
}
