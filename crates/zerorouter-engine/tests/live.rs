//! Live checks against the real providers (T060), run by the operator with their own
//! accounts: `ZR_LIVE=1 cargo test -p zerorouter-engine --test live -- text`. Skipped
//! unless `ZR_LIVE=1`. The accounts come from `$ZEROROUTER_HOME` (else `~/.0router`); a
//! provider without an account is skipped with a message.
//!
//! `ZR_LIVE_MODELS` (comma-separated `provider/model`) replaces the default text targets.
//!
//! `-- types` (T087) sends one non-text request per type and provider. Video is submitted
//! and polled once only with `ZR_LIVE_VIDEO=1`, since it is billed per clip.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{self, Answer, Media, MediaAnswer, TextRequest};
use zerorouter_engine::keys::AgentId;
use zerorouter_engine::records::{Outcome, RequestRecord};
use zerorouter_engine::state::Engine;
use zerorouter_registry::OperatorHome;
use zerorouter_registry::schema::ModelType;
use zerorouter_wire::codec::types::TypeValue;
use zerorouter_wire::codec::{request, response};
use zerorouter_wire::codec::response::ForClient;

/// One cheap target per text provider and wire. opencode's Messages wire takes `x-api-key`
/// and neither opencode wire gets fingerprint tools (R4): these requests carry no tools.
const TARGETS: &[&str] = &[
    "anthropic/claude-sonnet-4-20250514",
    "openrouter/openai/gpt-4o-mini",
    "opencode-zen/gpt-5-nano",
    "opencode-zen/claude-haiku-4-5",
    "opencode-go/deepseek-flash",
];

fn live() -> bool {
    std::env::var("ZR_LIVE").is_ok_and(|v| v == "1")
}

fn targets() -> Vec<String> {
    match std::env::var("ZR_LIVE_MODELS") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_owned()).collect(),
        _ => TARGETS.iter().map(|s| (*s).to_owned()).collect(),
    }
}

async fn settled(engine: &Engine, id: &str) -> RequestRecord {
    for _ in 0..600 {
        let r = engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("record {id} never finished");
}

/// Sends one request in `client`'s style and returns the answer's text and the record.
async fn send(engine: &Arc<Engine>, client: &str, target: &str, body: Value) -> (String, RequestRecord) {
    let st = engine.snapshot();
    let style = st.style(client).unwrap().clone();
    let ir = request::decode(&style, &body).unwrap();
    let id = zerorouter_engine::records::new_id();
    engine.records.insert(RequestRecord::new(id.clone(), "live".into(), style.id.clone()));
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let req = TextRequest {
        id: id.clone(),
        arrived: Instant::now(),
        client: style.clone(),
        body: body.clone(),
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_live", Some("live-1")),
        target: target.into(),
        stream,
        cancel: CancellationToken::new(),
        media: None,
    };
    let text = match engine.text(st.clone(), req).await {
        Ok(Answer::Whole { status, raw, answer, .. }) => {
            assert_eq!(status, 200, "{target}: {}", String::from_utf8_lossy(&raw));
            match *answer {
                ForClient::AsReceived { read } => read.or_else(|| response::decode(&style, &serde_json::from_slice(&raw).ok()?).ok()).map(|r| r.text()).unwrap_or_default(),
                ForClient::Rebuilt { read, .. } => read.text(),
            }
        }
        Ok(Answer::Events { rx, .. }) => match attempt::collect(&style, &body, rx).await {
            Ok(r) => r.text(),
            Err(e) => panic!("{target}: the stream ended in an error: {e:?}"),
        },
        Ok(Answer::Media(_)) => panic!("{target}: a non-text answer to a text request"),
        Err(f) => panic!("{target}: {} {}", f.status, f.message),
    };
    (text, settled(engine, &id).await)
}

#[tokio::test]
async fn text() {
    if !live() {
        eprintln!("skipped: set ZR_LIVE=1 to run the live checks");
        return;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let mut ran = 0;
    for target in targets() {
        let provider = target.split('/').next().unwrap();
        if st.accounts.for_provider(provider).all(|a| a.disabled) {
            eprintln!("{target}: skipped, no enabled {provider} account");
            continue;
        }
        for stream in [false, true] {
            let body = json!({
                "model": target,
                "max_tokens": 64,
                "stream": stream,
                "messages": [{ "role": "user", "content": "Reply with the single word: pong" }],
            });
            let started = Instant::now();
            let (text, rec) = send(&engine, "openai-chat", &target, body).await;
            let (input, output) = rec.usage.map_or((None, None), |u| (u.input, u.output));
            eprintln!(
                "{target} stream={stream}: {:?} in {} ms, input {input:?} output {output:?}, {} attempt(s)",
                text.trim(),
                started.elapsed().as_millis(),
                rec.attempts.len()
            );
            assert_eq!(rec.outcome, Outcome::Succeeded, "{target} stream={stream}: {rec:#?}");
            assert!(!text.trim().is_empty(), "{target} stream={stream}: an empty answer");
            assert!(input.is_some() && output.is_some(), "{target} stream={stream}: usage not recorded: {rec:#?}");
        }
        ran += 1;
    }
    assert!(ran > 0, "no provider had an account under {}", engine.home().path().display());
}

/// What a non-text request came back with.
enum Got {
    Value(TypeValue),
    Bytes(String, Vec<u8>),
    Job(String),
}

/// Sends one non-text request in the openai-chat style.
async fn send_media(engine: &Arc<Engine>, ty: ModelType, target: &str, body: Value) -> (Got, RequestRecord) {
    let st = engine.snapshot();
    let style = st.style("openai-chat").unwrap().clone();
    let codec = style.type_codec(ty).unwrap().clone();
    let input = codec.decode_request(&body).unwrap();
    let id = zerorouter_engine::records::new_id();
    engine.records.insert(RequestRecord::new(id.clone(), "live".into(), style.id.clone()));
    let job = ty == ModelType::Video;
    let req = TextRequest {
        id: id.clone(),
        arrived: Instant::now(),
        client: style,
        body,
        ir: Default::default(),
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_live", Some("live-1")),
        target: target.into(),
        stream: false,
        cancel: CancellationToken::new(),
        media: Some(Media { ty, codec, variant: None, input, voice: None, job }),
    };
    let got = match engine.text(st, req).await {
        Ok(Answer::Media(MediaAnswer::Value(v))) => Got::Value(v),
        Ok(Answer::Media(MediaAnswer::Bytes { content_type, mut rx })) => {
            let mut bytes = Vec::new();
            while let Some(chunk) = rx.recv().await {
                bytes.extend_from_slice(&chunk.unwrap_or_else(|e| panic!("{target}: the audio broke off: {e}")));
            }
            Got::Bytes(content_type, bytes)
        }
        Ok(Answer::Media(MediaAnswer::Job { id, .. })) => Got::Job(id),
        Ok(_) => panic!("{target}: a text answer to a non-text request"),
        Err(f) => panic!("{target}: {} {}", f.status, f.message),
    };
    let rec = if job { engine.records.get(&id).unwrap() } else { settled(engine, &id).await };
    (got, rec)
}

#[tokio::test]
async fn types() {
    if !live() {
        eprintln!("skipped: set ZR_LIVE=1 to run the live checks");
        return;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let has = |p: &str| {
        let ok = st.accounts.for_provider(p).any(|a| !a.disabled);
        if !ok {
            eprintln!("{p}: skipped, no enabled account");
        }
        ok
    };
    let mut ran = 0;
    if has("openrouter") {
        let (got, rec) = send_media(&engine, ModelType::Embeddings, "openrouter/openai/text-embedding-3-small", json!({"model": "x", "input": "pong"})).await;
        let Got::Value(v) = got else { panic!("embeddings: not a value") };
        let dims = v.items.first().and_then(|i| i.get("output.embedding")).and_then(Value::as_array).map_or(0, Vec::len);
        eprintln!("openrouter embeddings: {dims} dimensions, usage {:?}", rec.usage);
        assert!(dims > 0 && rec.outcome == Outcome::Succeeded, "{rec:#?}");

        let (got, rec) = send_media(&engine, ModelType::Image, "openrouter/openai/gpt-image-1", json!({"model": "x", "prompt": "a small red square", "size": "1024x1024"})).await;
        let Got::Value(v) = got else { panic!("image: not a value") };
        let item = v.items.first().expect("image: no data");
        eprintln!("openrouter image: b64 {} chars, url {:?}", item.str("output.b64_json").map_or(0, |s| s.len()), item.str("output.url"));
        assert_eq!(rec.outcome, Outcome::Succeeded, "{rec:#?}");

        let (got, rec) = send_media(&engine, ModelType::Tts, "openrouter/openai/gpt-4o-mini-tts", json!({"model": "x", "input": "pong"})).await;
        let Got::Bytes(ctype, bytes) = got else { panic!("openrouter speech: not audio bytes") };
        eprintln!("openrouter speech: {} bytes of {ctype}", bytes.len());
        assert!(!bytes.is_empty() && rec.outcome == Outcome::Succeeded, "{rec:#?}");

        if std::env::var("ZR_LIVE_VIDEO").is_ok_and(|v| v == "1") {
            let (got, rec) = send_media(&engine, ModelType::Video, "openrouter/google/veo-3.1", json!({"model": "x", "prompt": "a red ball rolling", "duration": 4})).await;
            let Got::Job(vj) = got else { panic!("video: not a job") };
            let (_, bindings, status) = engine.job_get(&st, &vj, "ak_live", "openai-chat").await.unwrap_or_else(|f| panic!("video poll: {} {}", f.status, f.message));
            eprintln!("openrouter video: {vj} is {status:?} ({bindings:?}), record {}", rec.id);
        }
        ran += 1;
    }
    if has("elevenlabs") {
        let (got, rec) = send_media(&engine, ModelType::Tts, "elevenlabs/eleven_flash_v2_5", json!({"model": "x", "input": "Hello from zero router."})).await;
        let Got::Bytes(ctype, audio) = got else { panic!("elevenlabs speech: not audio bytes") };
        eprintln!("elevenlabs speech: {} bytes of {ctype}", audio.len());
        assert!(!audio.is_empty() && rec.outcome == Outcome::Succeeded, "{rec:#?}");

        let file = zerorouter_wire::primitives::body::file(&audio, Some("speech.mp3"), Some(&ctype));
        let (got, rec) = send_media(&engine, ModelType::Stt, "elevenlabs/scribe_v2", json!({"model": "x", "file": file})).await;
        let Got::Value(v) = got else { panic!("elevenlabs transcription: not a value") };
        eprintln!("elevenlabs transcription: {:?}", v.str("output.text"));
        assert!(v.str("output.text").is_some_and(|t| !t.trim().is_empty()) && rec.outcome == Outcome::Succeeded, "{rec:#?}");
        ran += 1;
    }
    assert!(ran > 0, "no non-text provider had an account under {}", engine.home().path().display());
}
