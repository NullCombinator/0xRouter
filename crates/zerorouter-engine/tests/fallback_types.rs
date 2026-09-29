//! Non-text fallback (T077, US3-4, SC-007): a unified embeddings model and a unified TTS
//! model, each over two members, fall back exactly like text when the first one fails.

mod common;

use common::*;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{Answer, Failure, Media, MediaAnswer};
use zerorouter_engine::records::{AttemptKind, Outcome};
use zerorouter_engine::testkit::{MockUpstream, Step};
use zerorouter_registry::schema::ModelType;

/// A provider `id` with embeddings and speech endpoints on the openai-chat wire.
fn media_plugin(mock: &MockUpstream, id: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n\
         [endpoints.embeddings]\nurl = \"{}\"\nwire = \"openai-chat\"\nretry = {{ 500 = {{ retries = 0 }} }}\n\
         [endpoints.tts]\nurl = \"{}\"\nwire = \"openai-chat\"\nvoices = [\"alloy\"]\nretry = {{ 500 = {{ retries = 0 }} }}\n\
         [[models]]\nid = \"e1\"\nkind = \"embedding\"\n[[models]]\nid = \"s1\"\nkind = \"tts\"\n",
        mock.url(&format!("/{id}/embeddings")),
        mock.url(&format!("/{id}/audio/speech")),
    )
}

async fn two_members() -> Setup {
    let config = "[[unified_model]]\nname = \"ue\"\nmembers = [{ provider = \"alpha\", model = \"e1\" }, { provider = \"beta\", model = \"e1\" }]\n\
                  [[unified_model]]\nname = \"us\"\nmembers = [{ provider = \"alpha\", model = \"s1\" }, { provider = \"beta\", model = \"s1\" }]\n";
    setup(
        |m| vec![("alpha", media_plugin(m, "alpha")), ("beta", media_plugin(m, "beta"))],
        &[("alpha", "main"), ("beta", "main")],
        config,
    )
    .await
}

async fn send_media(s: &Setup, ty: ModelType, target: &str, body: Value) -> (String, Result<Answer, Failure>) {
    let mut req = request(s, "openai-chat", target, chat_body(target, false), "ak_test", CancellationToken::new());
    let codec = req.client.type_codec(ty).unwrap().clone();
    let input = codec.decode_request(&body).unwrap();
    req.body = body;
    req.media = Some(Media { ty, codec, variant: None, input, voice: None, job: false });
    let id = req.id.clone();
    (id, s.engine.text(s.engine.snapshot(), req).await)
}

#[tokio::test]
async fn embeddings_fall_back_to_the_next_member() {
    let s = two_members().await;
    s.mock.on("/alpha", [err(500)]);
    s.mock.on("/beta", [Step::json(200, json!({"object": "list", "data": [{"object": "embedding", "index": 0, "embedding": [0.5]}], "model": "e1", "usage": {"prompt_tokens": 2, "total_tokens": 2}}))]);
    let (id, res) = send_media(&s, ModelType::Embeddings, "ue", json!({"model": "ue", "input": "hi"})).await;
    let Ok(Answer::Media(MediaAnswer::Value(v))) = res else { panic!("expected a value") };
    assert_eq!(v.items[0].get("output.embedding"), Some(&json!([0.5])));
    assert_eq!(paths(&s), ["/alpha/embeddings", "/beta/embeddings"]);
    assert_eq!(s.mock.received()[1].json()["model"], "e1", "the member's own model id");
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(trail(&r), [t("alpha", "main", AttemptKind::Initial), t("beta", "main", AttemptKind::NextMember)]);
    assert_eq!(r.model_type, Some(ModelType::Embeddings));
    assert_eq!(r.unified_model.as_deref(), Some("ue"));
    assert_eq!(r.served_by.unwrap().provider, "beta");
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert_eq!(r.usage.unwrap().input, Some(2));
}

#[tokio::test]
async fn speech_falls_back_to_the_next_member() {
    let s = two_members().await;
    s.mock.on("/alpha", [err(500)]);
    s.mock.on("/beta", [Step::binary("audio/mpeg", &b"ID3"[..])]);
    let (id, res) = send_media(&s, ModelType::Tts, "us", json!({"model": "us", "input": "hi"})).await;
    let Ok(Answer::Media(MediaAnswer::Bytes { content_type, mut rx })) = res else { panic!("expected audio") };
    assert_eq!(content_type, "audio/mpeg");
    let mut audio = Vec::new();
    while let Some(chunk) = rx.recv().await {
        audio.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(audio, b"ID3");
    assert_eq!(paths(&s), ["/alpha/audio/speech", "/beta/audio/speech"]);
    assert_eq!(s.mock.received()[1].json()["voice"], "alloy", "the endpoint's default voice");
    let r = settled(&s, &id).await;
    assert_eq!(trail(&r), [t("alpha", "main", AttemptKind::Initial), t("beta", "main", AttemptKind::NextMember)]);
    assert_eq!(r.model_type, Some(ModelType::Tts));
    assert_eq!(r.outcome, Outcome::Succeeded);
}
