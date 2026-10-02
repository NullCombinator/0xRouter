//! No secret leaves where it belongs (T102, SC-006, US5-5, FR-033). The provider echoes
//! the account's secret in its error bodies and headers; neither that secret nor the agent
//! key may show up in the logs, any record, any client response, the operator socket's
//! answers (what the CLI prints), the headers the provider got beyond its auth header, or
//! the plugin-visible registry.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use common::{SECRET, chat_stream, server};
use nullrouter_engine::records::Query;
use nullrouter_engine::testkit::{Received, Step};
use nullrouter_server::operator;
use serde_json::json;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An error that quotes the credential the request carried, in the body and a header.
fn echo(status: u16, r: &Received) -> Step {
    let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    Step::Reply {
        status,
        headers: vec![("content-type".into(), "application/json".into()), ("x-debug-auth".into(), auth.clone())],
        body: Bytes::from(
            json!({"error": {"message": format!("invalid key {auth}"), "type": "invalid_request_error"}}).to_string(),
        ),
    }
}

fn reply(r: &Received) -> Step {
    let said = r.json()["messages"][0]["content"].as_str().unwrap_or_default().to_owned();
    match said.as_str() {
        "fail400" => echo(400, r),
        "fail401" => echo(401, r),
        "fail500" => echo(500, r),
        _ => chat_stream(),
    }
}

#[tokio::test]
async fn secrets_appear_nowhere_but_the_upstream_auth_header() {
    let logs = Captured::default();
    let sink = logs.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .init();

    let s = server().await;
    s.mock.respond(reply);
    let key = s.key.clone();
    let sentinels = [SECRET, key.as_str()];
    let c = reqwest::Client::new();
    let mut seen = Vec::new();
    for (door, said, stream) in [
        ("chat", "fail400", false),
        ("chat", "fail401", true),
        ("chat", "fail500", true),
        ("messages", "fail500", false),
        ("messages", "ok", true),
        ("chat", "ok", true),
    ] {
        let req = match door {
            "chat" => c.post(format!("{}/v1/chat/completions", s.base)).bearer_auth(&s.key),
            _ => c
                .post(format!("{}/v1/messages", s.base))
                .header("x-api-key", &s.key)
                .header("anthropic-version", "2023-06-01"),
        };
        let body = json!({"model": "mockco/m1", "max_tokens": 64, "stream": stream, "messages": [{"role": "user", "content": said}]});
        let r = req.body(body.to_string()).send().await.unwrap();
        let headers = format!("{:?}", r.headers());
        seen.push(format!("{door} {said}: {} {headers}\n{}", r.status(), r.text().await.unwrap()));
    }
    // A wrong key's refusal must not quote it either.
    let r = c
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(format!("{}x", s.key))
        .body("{}")
        .send()
        .await
        .unwrap();
    let headers = format!("{:?}", r.headers());
    seen.push(format!("wrong key: {headers}\n{}", r.text().await.unwrap()));

    let records = serde_json::to_string(&s.engine.records.query(&Query::default())).unwrap();
    assert!(records.contains("invalid key"), "the provider's errors reached the records: {records}");
    let mut socket = Vec::new();
    for op in [json!({"op": "records.list"}), json!({"op": "accounts.state"}), json!({"op": "reload"})] {
        socket.push(operator::handle(&s.engine, &op).await.to_string());
    }
    for r in s.engine.records.query(&Query::default()) {
        socket.push(operator::handle(&s.engine, &json!({"op": "records.get", "id": r.id})).await.to_string());
    }
    let upstream: Vec<String> = s
        .mock
        .received()
        .iter()
        .map(|r| {
            let beyond_auth: Vec<String> =
                r.headers.iter().filter(|(k, _)| *k != "authorization").map(|(k, v)| format!("{k}: {v:?}")).collect();
            format!("{} {}\n{}", r.path_and_query, beyond_auth.join("\n"), String::from_utf8_lossy(&r.body))
        })
        .collect();
    let registry = format!("{:?}", s.engine.snapshot().registry);
    let carried = s.mock.received().iter().all(|r| r.headers["authorization"] == format!("Bearer {SECRET}"));
    assert!(carried, "the provider gets the secret in its auth header");
    drop(s);
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.is_empty(), "no logs were captured");

    let places: [(&str, String); 6] = [
        ("logs", logs),
        ("records", records),
        ("client responses", seen.join("\n")),
        ("operator socket answers", socket.join("\n")),
        ("upstream requests beyond the auth header", upstream.join("\n")),
        ("registry", registry),
    ];
    let mut leaks = Vec::new();
    for (place, text) in &places {
        for secret in sentinels {
            if let Some(at) = text.find(secret) {
                let from = text[..at].char_indices().rev().nth(80).map_or(0, |(i, _)| i);
                leaks.push(format!("{place}: …{}…", &text[from..(at + secret.len()).min(text.len())]));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}
