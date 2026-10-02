//! A model used on a route of another type (T078, US3-5, FR-012): a 400 in the style's
//! shape naming both types, and nothing sent upstream.

mod common;

use common::server_with;
use nullrouter_engine::records::Outcome;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

fn embedco(mock: &nullrouter_engine::testkit::MockUpstream) -> (&'static str, String) {
    let toml = format!(
        "schema = 2\nid = \"embedco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n\
         [endpoints.embeddings]\nurl = \"{}\"\nwire = \"openai-chat\"\n[endpoints.tts]\nurl = \"{}\"\nwire = \"openai-chat\"\n\
         [[models]]\nid = \"emb\"\nkind = \"embedding\"\n",
        mock.url("/embedco/chat/completions"),
        mock.url("/embedco/embeddings"),
        mock.url("/embedco/audio/speech"),
    );
    ("embedco", toml)
}

async fn refused(path: &str, body: Value) -> (u16, Value) {
    let s = server_with(|m| vec![embedco(m)]).await;
    let r = reqwest::Client::new()
        .post(format!("{}{path}", s.base))
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert!(s.mock.received().is_empty(), "nothing goes upstream");
    assert_eq!(s.engine.records.get(&id).unwrap().outcome, Outcome::Failed);
    (status, body)
}

#[tokio::test]
async fn an_embeddings_model_on_the_chat_route() {
    let (status, body) = refused(
        "/v1/chat/completions",
        json!({"model": "embedco/emb", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, 400);
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("embeddings") && msg.contains("text"), "{msg}");
}

#[tokio::test]
async fn a_speech_request_to_an_embeddings_model() {
    let (status, body) =
        refused("/v1/audio/speech", json!({"model": "embedco/emb", "input": "hi", "voice": "alloy"})).await;
    assert_eq!(status, 400);
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("embeddings") && msg.contains("tts"), "{msg}");
}
