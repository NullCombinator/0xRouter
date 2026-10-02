//! Errors a client sees (T067, US2-6): the informational body in each style with
//! `retry-after`, keepalives while the engine retries a started stream, and a client that
//! leaves during a backoff.

mod common;

use std::time::Duration;

use common::{chat_stream, server, server_with};
use serde_json::{Value, json};
use zerorouter_engine::records::Outcome;
use zerorouter_engine::testkit::{MockUpstream, Step};
use zerorouter_server::relay::REQUEST_ID;

struct Call {
    path: &'static str,
    carrier: &'static str,
    body: Value,
}

fn calls() -> Vec<Call> {
    let msgs = json!([{"role": "user", "content": "hi"}]);
    vec![
        Call {
            path: "/v1/chat/completions",
            carrier: "authorization",
            body: json!({"model": "mockco/m1", "messages": msgs}),
        },
        Call {
            path: "/v1/messages",
            carrier: "x-api-key",
            body: json!({"model": "mockco/m1", "max_tokens": 64, "messages": msgs}),
        },
        Call { path: "/v1/responses", carrier: "authorization", body: json!({"model": "mockco/m1", "input": "hi"}) },
        Call {
            path: "/v1beta/models/mockco/m1:generateContent",
            carrier: "x-goog-api-key",
            body: json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}),
        },
    ]
}

fn post(base: &str, key: &str, c: &Call) -> reqwest::RequestBuilder {
    let v = if c.carrier == "authorization" { format!("Bearer {key}") } else { key.to_owned() };
    reqwest::Client::new()
        .post(format!("{base}{}", c.path))
        .header(c.carrier, v)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(c.body.to_string())
}

#[tokio::test]
async fn when_every_attempt_fails_each_style_gets_503_with_the_attempts() {
    for c in calls() {
        let s = server().await;
        // A 401 isn't retried and rests the account for 2 minutes.
        s.mock.push([Step::json(401, json!({"error": {"message": "bad key"}}))]);
        let r = post(&s.base, &s.key, &c).send().await.unwrap();
        assert_eq!(r.status(), 503, "{}", c.path);
        let retry_after: u64 = r.headers()["retry-after"].to_str().unwrap().parse().unwrap();
        assert!((110..=120).contains(&retry_after), "{}: {retry_after}", c.path);
        let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
        let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        let message = body["error"]["message"].as_str().unwrap_or_else(|| panic!("{}: {body}", c.path));
        assert!(
            message.starts_with(&format!("0router: no provider could serve mockco/m1 (record {id})")),
            "{}: {message}",
            c.path
        );
        assert!(message.contains("\nmockco/main m1: 401 "), "{}: {message}", c.path);
        // Messages puts the details at the top; the others inside `error` (client-surface.md).
        let details = if c.path == "/v1/messages" { &body["zerorouter"] } else { &body["error"]["zerorouter"] };
        assert_eq!(details["record_id"], id.as_str(), "{}: {body}", c.path);
        let a = &details["attempts"][0];
        assert_eq!(
            (a["provider"].as_str(), a["account"].as_str(), a["status"].as_u64()),
            (Some("mockco"), Some("main"), Some(401)),
            "{}",
            c.path
        );
        assert_eq!(a["class"], "auth");
        assert_eq!(s.engine.records.get(&id).unwrap().outcome, Outcome::Failed);
    }
}

/// A provider `alpha` whose 503s are retried once after `delay_ms`.
fn alpha(delay_ms: u64) -> impl FnOnce(&MockUpstream) -> Vec<(&'static str, String)> {
    move |m| {
        vec![(
            "alpha",
            format!(
                "schema = 2\nid = \"alpha\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\nretry = {{ 502 = {{ retries = 1, delay_ms = {delay_ms} }}, 503 = {{ retries = 1, delay_ms = {delay_ms} }} }}\n[[models]]\nid = \"m1\"\n",
                m.url("/alpha/chat/completions")
            ),
        )]
    }
}

#[tokio::test]
async fn a_started_stream_gets_keepalives_and_one_preamble_across_a_retry() {
    let s = server_with(alpha(50)).await;
    // The first answer opens, says nothing and drops: a break before output.
    let open = json!({"id": "c0", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]});
    let Step::Stream { status, headers, frames, .. } = Step::sse(&[(None, open)], false) else { unreachable!() };
    s.mock.on(
        "/alpha",
        [Step::Stream { status, headers, frames, every: Duration::from_millis(20), cut: true }, chat_stream()],
    );
    let c = Call {
        path: "/v1/messages",
        carrier: "x-api-key",
        body: json!({"model": "alpha/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}]}),
    };
    let r = post(&s.base, &s.key, &c).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    assert_eq!(text.matches("event: message_start").count(), 1, "{text}");
    assert!(text.contains("event: ping"), "a keepalive between the attempts: {text}");
    assert!(text.find("event: ping") < text.find("event: message_start"), "the preamble waits for content: {text}");
    assert!(text.contains("\"text\":\"Hel\""), "{text}");
    assert_eq!(s.mock.received().len(), 2);
}

#[tokio::test]
async fn a_client_that_leaves_during_a_backoff_cancels_it() {
    let s = server_with(alpha(1500)).await;
    s.mock.on("/alpha", [Step::json(503, json!({"error": {"message": "busy"}})), chat_stream()]);
    let c = Call {
        path: "/v1/messages",
        carrier: "x-api-key",
        body: json!({"model": "alpha/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}]}),
    };
    let gone = post(&s.base, &s.key, &c).timeout(Duration::from_millis(300)).send().await;
    assert!(gone.is_err(), "the client gave up during the backoff");
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert_eq!(s.mock.received().len(), 1, "no request after the client left");
    let recs = s.engine.records.query(&zerorouter_engine::records::Query::default());
    let rec = recs.iter().find(|r| r.target.as_deref() == Some("alpha/m1")).expect("a record");
    assert_eq!(rec.outcome, Outcome::Cancelled);
}
