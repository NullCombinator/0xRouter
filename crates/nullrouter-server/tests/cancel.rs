//! A client that goes away mid-stream (T047, SC-010, US1-6): the provider sees its
//! connection close within a second and the record says `cancelled`, for a native and a
//! translated stream.

mod common;

use std::time::{Duration, Instant};

use common::server;
use nullrouter_engine::records::Outcome;
use nullrouter_engine::testkit::Step;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::json;

async fn drop_mid_stream(path: &str, headers: &[(&str, &str)], body: serde_json::Value) {
    let s = server().await;
    let first = format!("data: {}\n\n", json!({"id": "up-1", "choices": [{"index": 0, "delta": {"content": "Hel"}}]}));
    s.mock.push([Step::StallAfter { frames: vec![first.into()], hold: Duration::from_secs(30) }]);
    let mut r = reqwest::Client::new().post(format!("{}{path}", s.base)).body(body.to_string());
    for (k, v) in headers {
        r = r.header(*k, v.replace("{key}", &s.key));
    }
    let mut r = r.send().await.unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let chunk = r.chunk().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&chunk).contains("data: "), "the first bytes arrived before the drop");

    drop(r);
    let dropped = Instant::now();
    while dropped.elapsed() < Duration::from_secs(1) {
        if !s.mock.disconnects().is_empty() && s.engine.records.get(&id).unwrap().outcome == Outcome::Cancelled {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "after 1 s: record {:?}; upstream disconnects {}",
        s.engine.records.get(&id).unwrap().outcome,
        s.mock.disconnects().len()
    );
}

#[tokio::test]
async fn a_native_stream_stops_upstream_when_the_client_goes() {
    let body = json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]});
    drop_mid_stream("/v1/chat/completions", &[("authorization", "Bearer {key}")], body).await;
}

#[tokio::test]
async fn a_translated_stream_stops_upstream_when_the_client_goes() {
    let body = json!({"model": "mockco/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}]});
    drop_mid_stream("/v1/messages", &[("x-api-key", "{key}"), ("anthropic-version", "2023-06-01")], body).await;
}
