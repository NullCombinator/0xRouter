//! Access keys and sessions over HTTP (T046): refusals in each style's error shape before
//! anything reaches a provider, Gemini's `?key=` carrier, and Claude Code sessions as agents.

mod common;

use common::{chat_whole, server};
use serde_json::{Value, json};
use zerorouter_server::relay::REQUEST_ID;

/// Each bundled style: the generate path, the key carrier header and its scheme prefix,
/// extra headers, a valid body, and where its error body keeps the message.
struct Style {
    id: &'static str,
    path: &'static str,
    carrier: &'static str,
    prefix: &'static str,
    extra: &'static [(&'static str, &'static str)],
    body: fn() -> Value,
    message: &'static str,
}

const STYLES: &[Style] = &[
    Style {
        id: "openai-chat",
        path: "/v1/chat/completions",
        carrier: "authorization",
        prefix: "Bearer ",
        extra: &[],
        body: || json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}),
        message: "/error/message",
    },
    Style {
        id: "anthropic-messages",
        path: "/v1/messages",
        carrier: "x-api-key",
        prefix: "",
        extra: &[("anthropic-version", "2023-06-01")],
        body: || json!({"model": "mockco/m1", "max_tokens": 8, "messages": [{"role": "user", "content": "hi"}]}),
        message: "/error/message",
    },
    Style {
        id: "openai-responses",
        path: "/v1/responses",
        carrier: "authorization",
        prefix: "Bearer ",
        extra: &[],
        body: || json!({"model": "mockco/m1", "input": "hi"}),
        message: "/error/message",
    },
    Style {
        id: "gemini",
        path: "/v1beta/models/mockco/m1:generateContent",
        carrier: "x-goog-api-key",
        prefix: "",
        extra: &[],
        body: || json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}),
        message: "/error/message",
    },
];

/// The style's own error shape, besides the message.
fn shaped(style: &str, body: &Value) -> bool {
    match style {
        "anthropic-messages" => body["type"] == "error" && body["error"]["type"] == "authentication_error",
        "gemini" => body["error"]["status"] == "UNAUTHENTICATED" && body["error"]["code"] == 401,
        _ => body["error"]["type"] == "invalid_request_error",
    }
}

#[tokio::test]
async fn missing_unknown_and_revoked_keys_are_refused_in_every_style() {
    let s = server().await;
    let c = reqwest::Client::new();
    let mut failures = Vec::new();
    for st in STYLES {
        let cases = [
            ("no key", None),
            ("unknown key", Some("zr-not-a-real-key-000000000000")),
            ("revoked key", Some(s.revoked.as_str())),
        ];
        for (case, key) in cases {
            let mut r = c.post(format!("{}{}", s.base, st.path)).body((st.body)().to_string());
            for (k, v) in st.extra {
                r = r.header(*k, *v);
            }
            if let Some(k) = key {
                r = r.header(st.carrier, format!("{}{k}", st.prefix));
            }
            let r = r.send().await.unwrap();
            let status = r.status().as_u16();
            let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap_or(Value::Null);
            let message = body.pointer(st.message).and_then(Value::as_str).unwrap_or_default();
            if status != 401 || !message.starts_with("0router:") || !shaped(st.id, &body) {
                failures.push(format!("{} {case}: {status} {body}", st.id));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(s.mock.received().is_empty(), "a refused request reached the provider");
}

#[tokio::test]
async fn gemini_takes_the_key_from_the_query() {
    let s = server().await;
    s.mock.push([chat_whole()]);
    let c = reqwest::Client::new();
    let body = json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}).to_string();
    let url = format!("{}/v1beta/models/mockco/m1:generateContent", s.base);
    let r = c.post(format!("{url}?key={}", s.key)).body(body.clone()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = c.post(format!("{url}?key={}", s.revoked)).body(body).send().await.unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(s.mock.received().len(), 1);
}

#[tokio::test]
async fn two_claude_code_sessions_under_one_key_are_two_agents() {
    let s = server().await;
    let c = reqwest::Client::new();
    let sessions = ["0b6f8c9e-1111-4a2b-9c3d-000000000001", "0b6f8c9e-2222-4a2b-9c3d-000000000002"];
    let mut agents = Vec::new();
    for session in sessions {
        s.mock.push([chat_whole()]);
        let user_id = format!("user_5e1f_account_8a2c_session_{session}");
        let body = json!({"model": "mockco/m1", "max_tokens": 8, "metadata": {"user_id": user_id}, "messages": [{"role": "user", "content": "hi"}]});
        let r = c
            .post(format!("{}/v1/messages", s.base))
            .header("x-api-key", &s.key)
            .header("anthropic-version", "2023-06-01")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
        agents.push(s.engine.records.get(&id).unwrap().agent.unwrap());
    }
    assert_eq!(agents[0].key, agents[1].key, "one key");
    assert_eq!(agents[0].session.as_deref(), Some(sessions[0]));
    assert_eq!(agents[1].session.as_deref(), Some(sessions[1]));
    assert_ne!(agents[0], agents[1], "two agents");
}
