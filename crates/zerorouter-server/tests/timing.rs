//! Recorded timings against what the client sees (T101, SC-005): a provider that waits
//! before its first content and again before its end. The record's TTFT and total are
//! taken at the server's socket writes, so they sit within 10 ms of the client's clock.

mod common;

use std::time::{Duration, Instant};

use axum::body::Bytes;
use common::server;
use futures_util::StreamExt;
use serde_json::{Value, json};
use zerorouter_engine::testkit::Step;
use zerorouter_server::relay::REQUEST_ID;

const GAP: Duration = Duration::from_millis(150);

/// A keepalive, then "Hel" one gap later, then "lo", the finish, usage and `[DONE]` a gap
/// apart: first content at 1 gap, the end at 5.
fn slow_stream() -> Step {
    let chunk = |delta: Value, finish: Value| json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    let data = |v: Value| Bytes::from(format!("data: {v}\n\n"));
    let frames = vec![
        Bytes::from_static(b": keepalive\n\n"),
        data(chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null)),
        data(chunk(json!({"content": "lo"}), Value::Null)),
        data(chunk(json!({}), json!("stop"))),
        data(
            json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2}}),
        ),
        Bytes::from_static(b"data: [DONE]\n\n"),
    ];
    Step::Stream {
        status: 200,
        headers: vec![("content-type".into(), "text/event-stream".into())],
        frames,
        every: GAP,
        cut: false,
    }
}

/// Sends a streamed request and returns the record id with the client's first-content and
/// end times, both from just before the send.
async fn measure(req: reqwest::RequestBuilder) -> (String, f64, f64) {
    let sent = Instant::now();
    let r = req.send().await.unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let (mut text, mut first) = (String::new(), None);
    let mut body = r.bytes_stream();
    while let Some(chunk) = body.next().await {
        text += &String::from_utf8_lossy(&chunk.unwrap());
        if first.is_none() && text.contains("Hel") {
            first = Some(sent.elapsed());
        }
    }
    let end = sent.elapsed();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    (id, ms(first.expect("no content arrived")), ms(end))
}

fn assert_close(what: &str, recorded: Option<f64>, measured: f64) {
    let recorded = recorded.unwrap_or_else(|| panic!("{what} missing from a streamed record"));
    assert!(
        (recorded - measured).abs() <= 10.0,
        "{what}: recorded {recorded:.1} ms, the client measured {measured:.1} ms"
    );
}

#[tokio::test]
async fn recorded_ttft_and_total_match_the_client_within_10ms() {
    let s = server().await;
    let c = reqwest::Client::new();
    // Connection setup happens before the request arrives; keep it out of the client's clock.
    c.get(format!("{}/v1/models", s.base)).bearer_auth(&s.key).send().await.unwrap();
    // Relayed frames (same style) and written events (translated).
    let requests = [
        c.post(format!("{}/v1/chat/completions", s.base))
            .bearer_auth(&s.key)
            .body(json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]}).to_string()),
        c.post(format!("{}/v1/messages", s.base))
            .header("x-api-key", &s.key)
            .header("anthropic-version", "2023-06-01")
            .body(json!({"model": "mockco/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}]}).to_string()),
    ];
    for req in requests {
        s.mock.push([slow_stream()]);
        let (id, first, end) = measure(req).await;
        assert!(
            first >= GAP.as_millis() as f64 && end >= 5.0 * GAP.as_millis() as f64,
            "the mock's delays: {first} {end}"
        );
        // The record is final once the writer's last byte went out.
        let mut rec = s.engine.records.get(&id).unwrap();
        for _ in 0..100 {
            if rec.total_ms.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
            rec = s.engine.records.get(&id).unwrap();
        }
        assert_close("ttft", rec.ttft_ms, first);
        assert_close("total", rec.total_ms, end);
    }
}
