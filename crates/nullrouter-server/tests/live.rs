//! The live view of requests in flight (spec 013, US2): SC-005 and the content check (FR-019).
//!
//! The router-overhead phase before a first attempt is covered by `live.rs`'s unit tests in the
//! engine: a request in that state lasts microseconds, too short to catch from outside.

mod common;

use std::time::{Duration, Instant};

use axum::body::Bytes;
use common::{SECRET, server};
use nullrouter_engine::testkit::Step;
use nullrouter_server::operator;
use serde_json::{Value, json};

fn data(v: Value) -> Bytes {
    Bytes::from(format!("data: {v}\n\n"))
}

fn frames(text: &str) -> Vec<Bytes> {
    let chunk = |delta: Value, finish: Value| {
        json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1",
               "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    };
    vec![
        data(chunk(json!({"role": "assistant", "content": text}), Value::Null)),
        data(chunk(json!({}), json!("stop"))),
        Bytes::from_static(b"data: [DONE]\n\n"),
    ]
}

fn phased(headers_delay: Duration, first_frame_delay: Duration, text: &str) -> Step {
    Step::Phased { headers_delay, first_frame_delay, frames: frames(text), every: Duration::ZERO }
}

fn chat(s: &common::Server, prompt: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": prompt}]}).to_string())
}

async fn snapshot(s: &common::Server) -> Value {
    let a = operator::handle(&s.engine, &json!({"op": "live.snapshot"})).await;
    assert_eq!(a["ok"], true, "{a}");
    a
}

fn in_flight(a: &Value) -> &Vec<Value> {
    a["in_flight"].as_array().unwrap()
}

fn phase_count(a: &Value, phase: &str) -> usize {
    in_flight(a).iter().filter(|r| r["attempt"]["phase"] == phase).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_request_in_flight_is_listed_in_its_phase_and_gone_soon_after_it_ends() {
    let s = server().await;
    assert_eq!(in_flight(&snapshot(&s).await).len(), 0, "nothing in flight gives an empty list");

    let park = Duration::from_millis(1500);
    for i in 0..200 {
        s.mock.push([if i % 2 == 0 { phased(park, Duration::ZERO, "hi") } else { phased(Duration::ZERO, park, "hi") }]);
    }
    let sent = Instant::now();
    let tasks: Vec<_> = (0..200)
        .map(|_| {
            let req = chat(&s, "hello");
            tokio::spawn(async move { req.send().await.unwrap().bytes().await.unwrap() })
        })
        .collect();

    // Half sit in headers, half in first token; none is missing from the list.
    let mut matched = false;
    while sent.elapsed() < Duration::from_millis(1200) {
        let a = snapshot(&s).await;
        let waiting = phase_count(&a, "headers") + phase_count(&a, "connect");
        if in_flight(&a).len() == 200 && phase_count(&a, "first_token") == 100 && waiting == 100 {
            matched = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(matched, "{:?}", snapshot(&s).await["in_flight"].as_array().map(Vec::len));

    for t in tasks {
        t.await.unwrap();
    }
    let ended = Instant::now();
    loop {
        if in_flight(&snapshot(&s).await).is_empty() {
            break;
        }
        assert!(ended.elapsed() < Duration::from_secs(1), "a finished request is still listed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_attempt_shows_with_the_first_in_finished() {
    let s = server().await;
    s.mock.push([Step::json(503, json!({"error": {"message": "overloaded"}}))]);
    s.mock.push([phased(Duration::ZERO, Duration::from_secs(2), "hi")]);
    let req = chat(&s, "hello");
    let task = tokio::spawn(async move { req.send().await.unwrap().bytes().await.unwrap() });
    let began = Instant::now();
    let found = loop {
        let a = snapshot(&s).await;
        if let Some(r) = in_flight(&a).iter().find(|r| r["attempt"]["n"] == 2) {
            break r.clone();
        }
        // The 503 is retried after 2 s (`classify::budget`).
        assert!(began.elapsed() < Duration::from_secs(8), "attempt 2 never showed: {a}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(found["finished"][0]["n"], 1);
    assert_eq!(found["finished"][0]["ended_in"], "headers");
    task.await.unwrap();
}

/// FR-019: the snapshot holds no prompt, answer, client header or account secret.
#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_carries_no_prompt_answer_header_or_secret() {
    let s = server().await;
    s.mock.push([phased(Duration::from_millis(600), Duration::from_millis(600), "ANSWER-MARKER-31")]);
    let req = chat(&s, "PROMPT-MARKER-17").header("x-client-marker", "HEADER-MARKER-44");
    let task = tokio::spawn(async move { req.send().await.unwrap().bytes().await.unwrap() });
    let mut seen = Vec::new();
    for _ in 0..16 {
        seen.push(snapshot(&s).await.to_string());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    task.await.unwrap();
    assert!(seen.iter().any(|t| t.contains("\"in_flight\":[{")), "the request was caught in flight");
    for text in &seen {
        for marker in ["PROMPT-MARKER-17", "ANSWER-MARKER-31", "HEADER-MARKER-44", SECRET] {
            assert!(!text.contains(marker), "{marker} leaked: {text}");
        }
    }
}

/// FR-020: a socket client that asks for a snapshot and never reads the reply doesn't slow
/// requests. (The reply here is small; the point is that the table's lock isn't held while
/// the reply is written.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_socket_client_that_never_reads_does_not_slow_requests() {
    use tokio::io::AsyncWriteExt;

    let s = server().await;
    let listener = operator::bind(s.engine.home()).unwrap();
    let engine = s.engine.clone();
    tokio::spawn(operator::serve(engine, listener, std::future::pending::<()>()));

    let fifty = |s: &common::Server| {
        for _ in 0..50 {
            s.mock.push([phased(Duration::from_millis(200), Duration::ZERO, "hi")]);
        }
        let tasks: Vec<_> = (0..50)
            .map(|_| {
                let req = chat(s, "hello");
                tokio::spawn(async move { req.send().await.unwrap().bytes().await.unwrap() })
            })
            .collect();
        async move {
            let began = Instant::now();
            for t in tasks {
                t.await.unwrap();
            }
            began.elapsed()
        }
    };
    let alone = fifty(&s).await;

    let mut idle = tokio::net::UnixStream::connect(operator::socket_path(s.engine.home())).await.unwrap();
    idle.write_all(b"{\"op\":\"live.snapshot\"}\n").await.unwrap();
    let with = fifty(&s).await;
    assert!(with < alone * 2 + Duration::from_millis(500), "50 requests took {with:?}, {alone:?} without the idle client");
    drop(idle);
}
