//! `chat_audio_delta_collect`: audio from a chat-completions stream with the audio
//! modality. 9router's openrouter TTS (`ttsProviders/openrouter.js`) joins every
//! `choices[0].delta.audio.data` string and decodes the result as one base64 text.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

#[derive(Debug, Default)]
pub struct AudioCollector {
    b64: String,
}

impl AudioCollector {
    /// Takes one parsed stream chunk.
    pub fn push(&mut self, chunk: &Value) {
        if let Some(d) = chunk.pointer("/choices/0/delta/audio/data").and_then(Value::as_str) {
            self.b64.push_str(d);
        }
    }

    /// The audio, or `None` when the stream carried none (9router: "returned no audio data").
    pub fn finish(self) -> Option<Vec<u8>> {
        if self.b64.is_empty() { None } else { STANDARD.decode(self.b64).ok() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn joins_the_deltas_before_decoding() {
        let mut c = AudioCollector::default();
        let whole = STANDARD.encode(b"RIFF-wave-bytes");
        let (a, b) = whole.split_at(5);
        for d in [json!({"choices": [{"delta": {"audio": {"data": a}}}]}), json!({"choices": [{"delta": {"content": "hi"}}]}), json!({"choices": [{"delta": {"audio": {"data": b}}}]})] {
            c.push(&d);
        }
        assert_eq!(c.finish().unwrap(), b"RIFF-wave-bytes");
        assert_eq!(AudioCollector::default().finish(), None);
    }
}
