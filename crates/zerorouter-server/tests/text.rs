//! Text generation over HTTP (T056): each bundled client style against a scripted
//! OpenAI-compatible provider, streamed and not, and errors.

mod common;

use common::{SECRET, chat_stream, chat_whole, server};
use serde_json::{Value, json};
use zerorouter_engine::records::{Outcome, Query};
use zerorouter_engine::testkit::Step;
use zerorouter_server::relay::REQUEST_ID;

fn json_body(v: &Value) -> String {
    v.to_string()
}

/// The `data:` payloads of an SSE body, `[DONE]` left out.
fn data_lines(text: &str) -> Vec<Value> {
    text.lines().filter_map(|l| l.strip_prefix("data: ")).filter(|d| *d != "[DONE]").map(|d| serde_json::from_str(d).unwrap()).collect()
}

#[tokio::test]
async fn openai_chat_whole_answer_goes_out_as_received() {
    let s = server().await;
    s.mock.push([chat_whole()]);
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(json_body(&json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]})))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(body["id"], "up-1", "same style: the provider's body");
    assert_eq!(body["choices"][0]["message"]["content"], "hi there");
    let sent = &s.mock.received()[0];
    assert_eq!(sent.headers["authorization"], format!("Bearer {SECRET}"), "the account's secret, not the access key");
    let rec = s.engine.records.get(&id).unwrap();
    assert_eq!(rec.outcome, Outcome::Succeeded);
    assert_eq!(rec.style, "openai-chat");
}

#[tokio::test]
async fn anthropic_client_streams_from_a_chat_provider() {
    let s = server().await;
    s.mock.push([chat_stream()]);
    let r = reqwest::Client::new()
        .post(format!("{}/v1/messages", s.base))
        .header("x-api-key", &s.key)
        .header("anthropic-version", "2023-06-01")
        .header("x-claude-code-session-id", "sess-42")
        .body(json_body(&json!({"model": "mockco/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}]})))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let text = r.text().await.unwrap();
    let events: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("event: ")).collect();
    assert_eq!(events.first(), Some(&"message_start"), "{text}");
    assert_eq!(events.last(), Some(&"message_stop"), "{text}");
    let deltas: String = data_lines(&text).iter().filter_map(|d| d["delta"]["text"].as_str().map(str::to_owned)).collect();
    assert_eq!(deltas, "Hello");

    let rec = s.engine.records.query(&Query::default()).into_iter().find(|r| r.id == id).unwrap();
    assert_eq!(rec.agent.unwrap().session.as_deref(), Some("sess-42"));
    assert_eq!(rec.outcome, Outcome::Succeeded);
    assert_eq!(rec.usage.unwrap().output, Some(2));
}

#[tokio::test]
async fn gemini_client_takes_the_model_from_the_path() {
    let s = server().await;
    s.mock.push([chat_stream(), chat_whole()]);
    let c = reqwest::Client::new();
    let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]});
    let r = c
        .post(format!("{}/v1beta/models/mockco/m1:streamGenerateContent?alt=sse", s.base))
        .header("x-goog-api-key", &s.key)
        .body(json_body(&body))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    let parts: String = data_lines(&text)
        .iter()
        .filter_map(|d| d["candidates"][0]["content"]["parts"][0]["text"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(parts, "Hello", "{text}");
    assert_eq!(s.mock.received()[0].json()["model"], "m1");

    let r = c.post(format!("{}/v1beta/models/mockco/m1:generateContent", s.base)).header("x-goog-api-key", &s.key).body(json_body(&body))
.send().await.unwrap();
    assert_eq!(r.status(), 200);
    let whole: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(whole["candidates"][0]["content"]["parts"][0]["text"], "hi there", "{whole}");
    assert_ne!(s.mock.received()[1].json()["stream"], true, "a whole request isn't streamed upstream");
}

#[tokio::test]
async fn errors_come_back_in_the_clients_style() {
    let s = server().await;
    s.mock.push([Step::json(400, json!({"error": {"message": "bad field"}}))]);
    let c = reqwest::Client::new();
    let url = format!("{}/v1/messages", s.base);
    let r = c.post(&url).header("x-api-key", &s.key).body(json_body(&json!({"model": "mockco/m1", "max_tokens": 8, "messages": [{"role": "user", "content": "hi"}]})))
.send().await.unwrap();
    assert_eq!(r.status(), 400);
    let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert!(body["error"]["message"].as_str().unwrap().starts_with("bad field (record "), "{body}");

    let r = c.post(&url).header("x-api-key", &s.key).body(json_body(&json!({"model": "nope/x", "max_tokens": 8, "messages": [{"role": "user", "content": "hi"}]})))
.send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = c.post(&url).header("x-api-key", &s.key).body(json_body(&json!({"max_tokens": 8, "messages": []})))
.send().await.unwrap();
    assert_eq!(r.status(), 400);
    let r = c.post(&url).header("x-api-key", &s.key).body("{not json").send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(s.mock.received().len(), 1, "only the first request reached the provider");
}
