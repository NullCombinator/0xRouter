//! Token counting over HTTP (T111, US6-3, US6-4). `counted` declares Anthropic's
//! `count_tokens` endpoint, so its count comes from the provider, retried on a transient
//! failure like generation. `routerish` declares none (openrouter's shape): the count is
//! 9router's estimate, marked in the header and the record. All three counting styles.

mod common;

use common::{Server, server_with};
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_server::count::ESTIMATE;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

fn counted(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "counted"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.text]
url = "{messages}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01" }}
auth = {{ header = "x-api-key", scheme = "raw" }}
[endpoints.text.token_count]
url = "{count}"
[[models]]
id = "c1"
"#,
        messages = mock.url("/counted/messages"),
        count = mock.url("/counted/count_tokens"),
    );
    ("counted", toml)
}

fn routerish(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        "schema = 2\nid = \"routerish\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"r1\"\n",
        mock.url("/routerish/chat/completions")
    );
    ("routerish", toml)
}

async fn start() -> Server {
    let s = server_with(|mock| vec![counted(mock), routerish(mock)]).await;
    s.mock.respond(|_| Step::json(200, json!({"input_tokens": 42})));
    s
}

/// Sends a count request in `style` for `model`; returns status, estimate header, record id and body.
async fn count(s: &Server, style: &str, model: &str, said: &str) -> (u16, bool, String, Value) {
    let c = reqwest::Client::new();
    let req = match style {
        "messages" => c
            .post(format!("{}/v1/messages/count_tokens", s.base))
            .header("x-api-key", &s.key)
            .header("anthropic-version", "2023-06-01")
            .body(json!({"model": model, "messages": [{"role": "user", "content": said}]}).to_string()),
        "responses" => c
            .post(format!("{}/v1/responses/input_tokens", s.base))
            .bearer_auth(&s.key)
            .body(json!({"model": model, "input": said}).to_string()),
        _ => c
            .post(format!("{}/v1beta/models/{model}:countTokens", s.base))
            .header("x-goog-api-key", &s.key)
            .body(json!({"contents": [{"role": "user", "parts": [{"text": said}]}]}).to_string()),
    };
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let estimated = r.headers().get(ESTIMATE).is_some_and(|v| v == "true");
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    (status, estimated, id, serde_json::from_slice::<Value>(&r.bytes().await.unwrap()).unwrap())
}

fn tokens(style: &str, body: &Value) -> u64 {
    let field = match style {
        "gemini" => "totalTokens",
        _ => "input_tokens",
    };
    body[field].as_u64().unwrap_or_else(|| panic!("{style}: no {field} in {body}"))
}

#[tokio::test]
async fn a_declared_count_comes_from_the_provider_in_every_style() {
    let s = start().await;
    for style in ["messages", "responses", "gemini"] {
        let (status, estimated, id, body) = count(&s, style, "counted/c1", "hello world").await;
        assert_eq!(status, 200, "{style}: {body}");
        assert!(!estimated, "{style}: a provider count isn't an estimate");
        assert_eq!(tokens(style, &body), 42, "{style}");
        if style == "responses" {
            assert_eq!(body["object"], "response.input_tokens");
        }
        let usage = s.engine.records.get(&id).unwrap().usage.unwrap();
        assert!(!usage.estimated, "{style}");
        assert_eq!(usage.input, Some(42), "{style}");
    }
    // Each count went to the count endpoint in the Messages shape, with no generation fields.
    let sent: Vec<_> = s.mock.received().into_iter().filter(|r| r.path_and_query == "/counted/count_tokens").collect();
    assert_eq!(sent.len(), 3);
    for r in &sent {
        let b = r.json();
        assert_eq!(b["model"], "c1", "{b}");
        assert!(b["messages"].is_array(), "{b}");
        for absent in ["stream", "max_tokens"] {
            assert!(b.get(absent).is_none(), "{absent} sent to count_tokens: {b}");
        }
        assert_eq!(r.headers["x-api-key"], common::SECRET);
    }
}

#[tokio::test]
async fn a_transient_count_failure_is_retried_like_generation() {
    let s = start().await;
    for style in ["messages", "responses", "gemini"] {
        s.mock.on("/counted/count_tokens", [Step::json(503, json!({"error": {"message": "overloaded"}}))]);
        let before = s.mock.received().len();
        let (status, _, id, body) = count(&s, style, "counted/c1", "hi").await;
        assert_eq!(status, 200, "{style}: {body}");
        assert_eq!(tokens(style, &body), 42);
        assert_eq!(s.mock.received().len() - before, 2, "{style}: one retry");
        let rec = s.engine.records.get(&id).unwrap();
        assert_eq!(rec.attempts.len(), 2, "{style}: {:#?}", rec.attempts);
    }
}

#[tokio::test]
async fn an_undeclared_count_is_the_marked_9router_estimate() {
    let s = start().await;
    let before = s.mock.received().len();

    // A Messages body is estimated as is: exactly the oracle's number.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/9router/count/cases.json");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let c = reqwest::Client::new();
    for case in file["data"]["cases"].as_array().unwrap() {
        let mut body = case["input"].clone();
        body["model"] = json!("routerish/r1");
        let r = c
            .post(format!("{}/v1/messages/count_tokens", s.base))
            .header("x-api-key", &s.key)
            .header("anthropic-version", "2023-06-01")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", case["name"]);
        assert_eq!(r.headers()[ESTIMATE], "true");
        let got: Value = serde_json::from_slice::<Value>(&r.bytes().await.unwrap()).unwrap();
        assert_eq!(got["input_tokens"], case["input_tokens"], "{}", case["name"]);
    }

    // The other styles are estimated through the Messages shape.
    for style in ["messages", "responses", "gemini"] {
        let (status, estimated, id, body) = count(&s, style, "routerish/r1", "hello world").await;
        assert_eq!(status, 200, "{style}: {body}");
        assert!(estimated, "{style}: the estimate header");
        assert_eq!(tokens(style, &body), 3, "{style}: {body}");
        let rec = s.engine.records.get(&id).unwrap();
        let usage = rec.usage.unwrap();
        assert!(usage.estimated, "{style}: the record marks the estimate");
        assert_eq!(usage.input, Some(3));
    }
    assert_eq!(s.mock.received().len(), before, "an estimate calls no provider");
}
