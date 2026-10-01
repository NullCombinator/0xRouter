//! Header forwarding and the security floor (T120, US7-1 to US7-3, FR-039). The `anth`
//! plugin declares `anthropic-beta` upstream (appended to its static value) and
//! `request-id`, `retry-after` and `anthropic-ratelimit-*` back to the client.

mod common;

use common::{SECRET, Setup, request, setup};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::Reply;
use zerorouter_engine::testkit::{MockUpstream, Step};

fn anth(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "anth"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.text]
url = "{}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01", "anthropic-beta" = "static-1" }}
auth = {{ header = "x-api-key", scheme = "raw" }}
[forwarding.to_upstream]
headers = [{{ name = "anthropic-beta", merge = "append_csv" }}]
[forwarding.to_client]
headers = ["request-id", "retry-after", "anthropic-ratelimit-*"]
[[models]]
id = "m1"
"#,
        mock.url("/anth/messages")
    );
    ("anth", toml)
}

fn answer() -> Step {
    Step::json(
        200,
        json!({"id": "msg_1", "type": "message", "role": "assistant", "model": "m1", "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn", "usage": {"input_tokens": 3, "output_tokens": 1}}),
    )
    .with_header("request-id", "req_mock_1")
    .with_header("anthropic-ratelimit-requests-remaining", "99")
    .with_header("anthropic-ratelimit-tokens-reset", &format!("{SECRET}-anth-a1"))
    .with_header("set-cookie", "session=1")
    .with_header("x-internal", "1")
}

async fn start() -> Setup {
    let s = setup(|m| vec![anth(m)], &[("anth", "a1")], "").await;
    s.mock.respond(|_| answer());
    s
}

fn client_headers(beta: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in [
        ("anthropic-beta", beta),
        ("x-undeclared", "1"),
        ("x-api-key", "0r-client-key"),
        ("authorization", "Bearer 0r-client-key"),
        ("cookie", "c=1"),
    ] {
        h.insert(k, HeaderValue::from_str(v).unwrap());
    }
    h
}

async fn send(s: &Setup, client: &str, body: Value, headers: HeaderMap) -> Reply {
    let mut req = request(s, client, "anth/m1", body, "ak_test", CancellationToken::new());
    req.headers = headers;
    s.engine.reply(s.engine.snapshot(), req).await.unwrap_or_else(|f| panic!("{f:?}"))
}

fn chat() -> Value {
    json!({"model": "anth/m1", "messages": [{"role": "user", "content": "hi"}]})
}

fn messages() -> Value {
    json!({"model": "anth/m1", "max_tokens": 16, "messages": [{"role": "user", "content": "hi"}]})
}

/// Nothing the client authenticated with, and no cookie, reaches the provider.
fn assert_floor(sent: &HeaderMap) {
    assert_eq!(sent["x-api-key"], format!("{SECRET}-anth-a1"), "the account's secret, not the client's key");
    assert!(sent.get("authorization").is_none(), "{sent:?}");
    assert!(sent.get("cookie").is_none(), "{sent:?}");
}

#[tokio::test]
async fn a_cross_style_client_sends_only_the_declared_headers() {
    let s = start().await;
    let reply = send(&s, "openai-chat", chat(), client_headers("client-beta")).await;
    let sent = &s.mock.received()[0].headers;
    assert_eq!(sent["anthropic-beta"], "static-1,client-beta", "appended to the static value");
    assert!(sent.get("x-undeclared").is_none(), "an undeclared header crossed styles");
    assert_floor(sent);

    let got: Vec<(String, String)> =
        reply.headers.iter().map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_owned())).collect();
    assert!(got.contains(&("request-id".into(), "req_mock_1".into())), "{got:?}");
    assert!(got.contains(&("anthropic-ratelimit-requests-remaining".into(), "99".into())), "{got:?}");
    for (n, _) in &got {
        assert!(
            !["set-cookie", "x-internal", "anthropic-ratelimit-tokens-reset"].contains(&n.as_str()),
            "{n} reached the client"
        );
    }
}

#[tokio::test]
async fn a_same_style_client_sends_every_header_but_the_floor() {
    let s = start().await;
    let reply = send(&s, "anthropic-messages", messages(), client_headers("client-beta")).await;
    let sent = &s.mock.received()[0].headers;
    assert_eq!(sent["x-undeclared"], "1", "same-style passes undeclared headers");
    assert_eq!(sent["anthropic-beta"], "static-1,client-beta", "the merge rule still applies");
    assert_floor(sent);
    assert!(reply.headers.iter().any(|(n, _)| n == "request-id"));
    assert!(!reply.headers.iter().any(|(n, _)| n == "set-cookie"));
}

#[tokio::test]
async fn a_value_holding_a_secret_is_dropped_and_cr_lf_never_forms_a_header() {
    let s = start().await;
    send(&s, "openai-chat", chat(), client_headers(&format!("beta-{SECRET}-anth-a1"))).await;
    let sent = &s.mock.received()[0].headers;
    assert_eq!(sent["anthropic-beta"], "static-1", "the secret-bearing value was dropped");
    // CR/LF can't reach a header map at all; the engine's check is the second line.
    assert!(HeaderValue::from_bytes(b"a\r\nx-injected: 1").is_err());
}
