//! Signed-in accounts serve like key accounts (spec 005 T026; FR-004a, FR-004c, FR-007,
//! FR-008; research R6, R7, R8). The plugins are the bundled anthropic, xai and grok-cli
//! files with their hosts pointed at the mock; every account is a sign-in account.
//!
//! xai video jobs are covered in `video_jobs.rs`.

mod common;

use common::{SECRET, Server, bundled_at_mock, reply_by_wire, signin_server};
use nullrouter_engine::records::{Outcome, RequestRecord};
use nullrouter_engine::testkit::{Received, Step};
use nullrouter_registry::schema::ModelType;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

/// The unified names equal the upstream ids, so a client's body names the model exactly as
/// the provider does and a same-style body can go through byte for byte.
const CONFIG: &str = r#"[plugin_decisions]
anthropic = "replace"
xai = "replace"
"grok-cli" = "replace"
[[unified_model]]
name = "claude-sonnet-4-20250514"
members = [{ provider = "anthropic", model = "claude-sonnet-4-20250514" }]
[[unified_model]]
name = "grok-4"
members = [{ provider = "xai", model = "grok-4" }]
[[unified_model]]
name = "grok-4.5"
members = [{ provider = "grok-cli", model = "grok-4.5" }]
[[unified_model]]
name = "grok-4.5-high"
members = [{ provider = "grok-cli", model = "grok-4.5-high" }]
"#;

async fn server() -> Server {
    let s = signin_server(
        |m| ["anthropic", "xai", "grok-cli"].map(|id| (id, bundled_at_mock(m, id))).into_iter().collect(),
        &[("anthropic", "max"), ("xai", "main"), ("grok-cli", "work")],
        CONFIG,
    )
    .await;
    s.mock.respond(reply_by_wire);
    s
}

/// The route and body for a `style` client asking `model`.
fn request(s: &Server, style: &str, model: &str, stream: bool) -> (reqwest::RequestBuilder, String) {
    let c = reqwest::Client::new();
    let (path, body) = match style {
        "openai-chat" => (
            "/v1/chat/completions",
            json!({"model": model, "stream": stream, "messages": [{"role": "user", "content": "hi"}]}),
        ),
        "anthropic-messages" => (
            "/v1/messages",
            json!({"model": model, "max_tokens": 64, "stream": stream, "messages": [{"role": "user", "content": "hi"}]}),
        ),
        "openai-responses" => ("/v1/responses", json!({"model": model, "stream": stream, "input": "hi"})),
        _ => unreachable!(),
    };
    let body = body.to_string();
    let b = c.post(format!("{}{path}", s.base)).header("content-type", "application/json").body(body.clone());
    let b = match style {
        "anthropic-messages" => b.header("x-api-key", &s.key).header("anthropic-version", "2023-06-01"),
        _ => b.bearer_auth(&s.key),
    };
    (b, body)
}

fn record(s: &Server, r: &reqwest::Response) -> RequestRecord {
    let id = r.headers()[REQUEST_ID].to_str().unwrap();
    s.engine.records.get(id).unwrap()
}

async fn settled(s: &Server, id: &str) -> RequestRecord {
    for _ in 0..400 {
        let r = s.engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("record {id} never finished");
}

/// The request the mock got last.
fn last(s: &Server) -> Received {
    s.mock.received().pop().expect("the provider got the request")
}

/// What every sign-in request carries: the token as a bearer, never an `x-api-key` (the
/// client's 0router key included).
fn assert_token(got: &Received, provider: &str, account: &str) {
    assert_eq!(got.headers["authorization"], format!("Bearer {SECRET}-{provider}-{account}"));
    assert!(got.headers.get("x-api-key").is_none(), "{provider}: no x-api-key");
}

#[tokio::test]
async fn every_client_style_is_served_streamed_and_whole() {
    let s = server().await;
    let targets = [
        ("anthropic", "max", "anthropic/claude-sonnet-4-20250514", "/v1/messages"),
        ("xai", "main", "xai/grok-4", "/v1/chat/completions"),
        ("grok-cli", "work", "grok-cli/grok-4.5", "/v1/responses"),
    ];
    for (provider, account, model, path) in targets {
        for style in ["openai-chat", "anthropic-messages", "openai-responses"] {
            for stream in [false, true] {
                let what = format!("{style} → {model}, stream {stream}");
                let (req, _) = request(&s, style, model, stream);
                let r = req.send().await.unwrap();
                assert_eq!(r.status(), 200, "{what}");
                let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
                let text = r.text().await.unwrap();
                assert!(text.contains("Hel"), "{what}: {text}");
                let rec = settled(&s, &id).await;
                assert_eq!(rec.outcome, Outcome::Succeeded, "{what}");
                let by = rec.served_by.unwrap();
                assert_eq!((by.provider.as_str(), by.account.as_deref()), (provider, Some(account)), "{what}");
                let got = last(&s);
                assert_eq!(got.path_and_query, path, "{what}");
                assert_token(&got, provider, account);
            }
        }
    }
}

#[tokio::test]
async fn identity_headers_arrive_filled() {
    let s = server().await;
    let (req, _) = request(&s, "anthropic-messages", "anthropic/claude-sonnet-4-20250514", false);
    let r = req.header("anthropic-beta", "prompt-caching-2024-07-31").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let got = last(&s);
    assert_eq!(got.headers["anthropic-beta"], "prompt-caching-2024-07-31,oauth-2025-04-20", "merged, not replaced");
    assert_eq!(got.headers["user-agent"], "claude-cli/2.1.280 (external, sdk-cli)");
    assert_eq!(got.headers["x-app"], "cli");

    let (req, _) = request(&s, "openai-responses", "grok-cli/grok-4.5", true);
    let r = req.header("x-session-id", "sess-42").send().await.unwrap();
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    let got = last(&s);
    let h = |n: &str| got.headers.get(n).map(|v| v.to_str().unwrap().to_owned()).unwrap_or_default();
    assert_eq!(h("user-agent"), "grok-shell/0.2.99 (linux; x86_64)");
    assert_eq!(h("x-grok-client-identifier"), "grok-shell");
    assert_eq!(h("x-grok-client-version"), "0.2.99");
    assert!(!h("x-grok-session-id").is_empty(), "the agent's session id");
    assert_eq!(h("x-grok-conv-id"), h("x-grok-session-id"));
    assert_eq!(h("x-grok-req-id").len(), 36, "a fresh UUID");
    assert_eq!(h("x-grok-turn-idx"), "1");
    assert_eq!(h("x-grok-model-override"), "grok-4.5");
    assert_eq!(h("x-email"), "work@example.com");
    assert_eq!(h("x-userid"), "user-work");
    assert_eq!(h("x-grok-agent-id"), s.engine.install_id().unwrap());

    let (req, _) = request(&s, "openai-chat", "xai/grok-4", false);
    req.send().await.unwrap();
    let got = last(&s);
    assert!(got.headers.get("x-grok-agent-id").is_none(), "xai declares no identity");
}

#[tokio::test]
async fn a_same_style_body_goes_upstream_byte_for_byte() {
    let s = server().await;
    let cases = [
        ("anthropic-messages", "claude-sonnet-4-20250514", false),
        ("anthropic-messages", "claude-sonnet-4-20250514", true),
        ("openai-chat", "grok-4", false),
        ("openai-responses", "grok-4.5", true),
    ];
    for (style, model, stream) in cases {
        let (req, sent) = request(&s, style, model, stream);
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), 200, "{style} {model}");
        let rec = record(&s, &r);
        r.bytes().await.unwrap();
        let got = last(&s);
        assert_eq!(String::from_utf8_lossy(&got.body), sent, "{style} {model} stream {stream}");
        let rec = settled(&s, &rec.id).await;
        assert!(rec.attempts.iter().all(|a| a.forced.is_empty()), "{style} {model}");
    }
}

#[tokio::test]
async fn an_effort_suffixed_model_differs_only_by_reasoning_effort() {
    let s = server().await;
    let (req, sent) = request(&s, "openai-responses", "grok-4.5-high", true);
    let r = req.send().await.unwrap();
    assert_eq!(r.status(), 200);
    let rec = record(&s, &r);
    r.bytes().await.unwrap();
    let got = last(&s);
    let mut want: Value = serde_json::from_str(&sent).unwrap();
    want["model"] = json!("grok-4.5");
    want["reasoning"] = json!({"effort": "high"});
    assert_eq!(String::from_utf8_lossy(&got.body), want.to_string(), "the upstream id, then reasoning.effort");
    let rec = settled(&s, &rec.id).await;
    assert_eq!(rec.attempts[0].forced, [("reasoning.effort".to_owned(), json!("high"))]);
    assert_eq!(got.headers["x-grok-model-override"], "grok-4.5");
}

#[tokio::test]
async fn xai_images_are_served_and_recorded_with_their_type() {
    let s = server().await;
    s.mock.on("/v1/images/generations", [Step::json(200, json!({"created": 1, "data": [{"b64_json": "aGk="}]}))]);
    let body = json!({"model": "xai/grok-2-image-1212", "prompt": "a cat", "response_format": "b64_json"});
    let r = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", s.base))
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let rec = record(&s, &r);
    let answer: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(answer["data"][0]["b64_json"], "aGk=");
    let got = last(&s);
    assert_eq!(got.path_and_query, "/v1/images/generations");
    assert_eq!(got.json()["prompt"], "a cat");
    assert_token(&got, "xai", "main");
    let rec = settled(&s, &rec.id).await;
    assert_eq!(rec.model_type, Some(ModelType::Image));
    assert_eq!(rec.outcome, Outcome::Succeeded);
    assert_eq!(rec.served_by.unwrap().account.as_deref(), Some("main"));
}
