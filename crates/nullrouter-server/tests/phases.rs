//! Phase times in request records (spec 013, US1): SC-001, SC-002, SC-003.
//!
//! The mock's `Step::Phased` delays each phase in turn and the record's phases must show the
//! delay in the right phase of the right attempt. The mock can't delay a client's connect (the
//! kernel completes the handshake), so the connect delay itself is covered by the engine's
//! connector-layer tests; here a new connection must read as one.
//!
//! Not covered here yet: the 0router-side delay (needs a slow `outgoing` hook in the testkit), a
//! broken stream resumed by a second attempt, an async media job, and the refresh case of SC-003.
//! The refresh case needs the engine's sign-in kit (`tests/signin_kit`), which this crate lacks.

mod common;

use std::time::Duration;

use axum::body::Bytes;
use common::server;
use futures_util::StreamExt;
use nullrouter_engine::phases::{self, Phase, PhaseValue};
use nullrouter_engine::records::{AttemptKind, Connection, RequestRecord};
use nullrouter_engine::testkit::{Step, read_slowly};
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const HEADERS: Duration = Duration::from_millis(150);
const FIRST: Duration = Duration::from_millis(200);
const EVERY: Duration = Duration::from_millis(100);

fn chunk(delta: Value, finish: Value) -> Value {
    json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1",
           "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
}

fn data(v: Value) -> Bytes {
    Bytes::from(format!("data: {v}\n\n"))
}

/// "Hel", "lo", the finish, usage and `[DONE]`: five frames after the first.
fn frames(first_pings: usize) -> Vec<Bytes> {
    let mut out: Vec<Bytes> = (0..first_pings).map(|_| Bytes::from_static(b": keepalive\n\n")).collect();
    out.push(data(chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null)));
    out.push(data(chunk(json!({"content": "lo"}), Value::Null)));
    out.push(data(chunk(json!({}), json!("stop"))));
    out.push(data(
        json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [],
               "usage": {"prompt_tokens": 5, "completion_tokens": 2}}),
    ));
    out.push(Bytes::from_static(b"data: [DONE]\n\n"));
    out
}

fn phased(headers_delay: Duration, first_frame_delay: Duration, every: Duration, frames: Vec<Bytes>) -> Step {
    Step::Phased { headers_delay, first_frame_delay, frames, every }
}

fn chat(s: &common::Server, stream: bool) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(json!({"model": "mockco/m1", "stream": stream, "messages": [{"role": "user", "content": "hi"}]}).to_string())
}

/// Sends a request, reads it to the end, and returns the finished record.
async fn run(s: &common::Server, req: reqwest::RequestBuilder) -> RequestRecord {
    let r = req.send().await.unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    r.bytes().await.unwrap();
    finished(s, &id).await
}

async fn finished(s: &common::Server, id: &str) -> RequestRecord {
    for _ in 0..200 {
        if let Some(rec) = s.engine.records.get(id)
            && rec.total_ms.is_some()
            && rec.attempts.last().is_some_and(|a| a.ended.is_some())
        {
            return rec;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("record {id} never finished");
}

fn ms(p: &phases::AttemptPhases, ph: Phase) -> f64 {
    match p.value(ph) {
        PhaseValue::Ms(v) => v,
        other => panic!("{ph:?} is {other:?}"),
    }
}

/// SC-001's tolerance: 5 % of the delay, at least 10 ms here (CI runners add scheduling noise).
fn close(what: &str, got: f64, want: Duration) {
    let want = want.as_secs_f64() * 1e3;
    let tol = (want * 0.05).max(10.0);
    assert!((got - want).abs() <= tol, "{what}: {got:.1} ms, expected {want:.1} ± {tol:.1}");
}

fn sum_matches(rec: &RequestRecord) {
    let all = phases::of(rec);
    let sum: f64 = all.iter().map(|p| p.sum()).sum();
    let total = rec.total_ms.unwrap();
    assert!((sum - total).abs() <= 1.0, "phases add up to {sum:.2} ms, total is {total:.2} ms");
}

fn ttft_matches(rec: &RequestRecord) {
    let p = &phases::of(rec)[0];
    let up_to: f64 = [Phase::RouterOverhead, Phase::Connect, Phase::Headers, Phase::FirstToken]
        .into_iter()
        .filter_map(|ph| p.value(ph).ms())
        .sum();
    let ttft = rec.ttft_ms.unwrap();
    assert!((up_to - ttft).abs() <= 1.0, "phases up to the first output add up to {up_to:.2} ms, ttft is {ttft:.2} ms");
}

#[tokio::test]
async fn each_injected_delay_shows_in_its_phase_and_the_phases_add_up() {
    let s = server().await;
    s.mock.push([phased(HEADERS, FIRST, EVERY, frames(0))]);
    let rec = run(&s, chat(&s, true)).await;
    assert_eq!(rec.attempts.len(), 1);
    let t = rec.attempts[0].timing.as_ref().expect("timing is recorded");
    assert_eq!(t.connection, Connection::New);
    let p = &phases::of(&rec)[0];
    assert!(ms(p, Phase::Connect) >= 0.0);
    close("headers", ms(p, Phase::Headers), HEADERS);
    close("first token", ms(p, Phase::FirstToken), FIRST);
    // The four frames after the first, each an `every` apart.
    close("generation", ms(p, Phase::Generation), EVERY * 4);
    assert_eq!(p.ended_in, None);
    sum_matches(&rec);
    ttft_matches(&rec);
}

#[tokio::test]
async fn pings_before_the_first_content_do_not_end_first_token() {
    let s = server().await;
    s.mock.push([phased(Duration::ZERO, FIRST, EVERY, frames(2))]);
    let rec = run(&s, chat(&s, true)).await;
    let p = &phases::of(&rec)[0];
    // The two pings come with the first frame's wait; the first token is the "Hel" frame after them.
    assert!(ms(p, Phase::FirstToken) >= FIRST.as_secs_f64() * 1e3 - 10.0);
    sum_matches(&rec);
    ttft_matches(&rec);
}

#[tokio::test]
async fn the_second_request_reuses_the_connection() {
    let s = server().await;
    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::ZERO, frames(0))]);
    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::ZERO, frames(0))]);
    let first = run(&s, chat(&s, true)).await;
    let second = run(&s, chat(&s, true)).await;
    assert_eq!(first.attempts[0].timing.as_ref().unwrap().connection, Connection::New);
    let t = second.attempts[0].timing.as_ref().unwrap();
    assert_eq!(t.connection, Connection::Reused);
    assert_eq!(t.connected, None);
    assert_eq!(phases::of(&second)[0].value(Phase::Connect), PhaseValue::NotApplicable);
    assert_eq!(s.mock.connections(), 1);
}

#[tokio::test]
async fn a_failed_attempt_ends_in_headers_and_leaves_the_later_phases_empty() {
    let s = server().await;
    s.mock.push([Step::json(503, json!({"error": {"message": "overloaded"}}))]);
    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::ZERO, frames(0))]);
    let rec = run(&s, chat(&s, true)).await;
    assert!(rec.attempts.len() >= 2, "the 503 was retried");
    let all = phases::of(&rec);
    assert_eq!(all[0].ended_in, Some(Phase::Headers));
    for ph in [Phase::FirstToken, Phase::Generation, Phase::Delivery] {
        assert_eq!(all[0].value(ph), PhaseValue::NotApplicable, "{ph:?}");
    }
    let last = rec.attempts.len() - 1;
    // A 503 gets three same-account retries after 2 s (`classify::budget`).
    assert_eq!(rec.attempts[last].kind, AttemptKind::SameAccountRetry);
    close("retry wait", ms(&all[last], Phase::RetryWait), Duration::from_secs(2));
    assert_eq!(all[last].ended_in, None);
    sum_matches(&rec);
}

#[tokio::test]
async fn headers_flushed_with_the_first_frame_are_one_wait() {
    let s = server().await;
    s.mock.push([phased(HEADERS, Duration::ZERO, EVERY, frames(0))]);
    let rec = run(&s, chat(&s, true)).await;
    let p = &phases::of(&rec)[0];
    assert!(p.merged_wait, "{:?}", rec.attempts[0].timing);
    assert_eq!(p.value(Phase::FirstToken), PhaseValue::NotApplicable);
    close("waiting for provider", ms(p, Phase::Headers), HEADERS);
    let json = serde_json::to_value(p).unwrap();
    assert!(json.get("waiting_for_provider").is_some() && json.get("headers").is_none());
    sum_matches(&rec);
}

#[tokio::test]
async fn a_client_that_stops_reading_makes_the_wait_delivery_not_generation() {
    let s = server().await;
    // Enough bytes to fill the socket buffers and the relay channel.
    let big = "x".repeat(16 * 1024);
    let mut frames: Vec<Bytes> = (0..1500).map(|_| data(chunk(json!({"content": big}), Value::Null))).collect();
    frames.push(Bytes::from_static(b"data: [DONE]\n\n"));
    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::ZERO, frames)]);
    let r = chat(&s, true).send().await.unwrap();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(700)).await;
    read_slowly(Box::pin(r.bytes_stream().map(|c| c.map_err(|_| ()))), Duration::ZERO).await;
    let rec = finished(&s, &id).await;
    let t = rec.attempts[0].timing.as_ref().unwrap();
    assert!(t.blocked_ms >= 400.0, "blocked {} ms", t.blocked_ms);
    let p = &phases::of(&rec)[0];
    assert!(ms(p, Phase::Delivery) >= 400.0, "delivery {} ms", ms(p, Phase::Delivery));
    sum_matches(&rec);
}

#[tokio::test]
async fn a_thinking_delta_ends_first_token_before_any_text() {
    let s = server().await;
    let mut fr: Vec<Bytes> = (0..2).map(|_| Bytes::from_static(b": keepalive\n\n")).collect();
    fr.push(data(chunk(json!({"role": "assistant", "reasoning_content": "hm"}), Value::Null)));
    fr.extend(frames(0));
    s.mock.push([phased(Duration::ZERO, FIRST, EVERY, fr)]);
    let rec = run(&s, chat(&s, true)).await;
    let p = &phases::of(&rec)[0];
    // Two pings, then the thinking delta: FIRST for the first ping and EVERY for each frame after.
    close("first token", ms(p, Phase::FirstToken), FIRST + EVERY * 2);
    sum_matches(&rec);
    ttft_matches(&rec);
}

#[tokio::test]
async fn a_whole_json_answer_has_no_generation() {
    let s = server().await;
    s.mock.push([common::chat_whole()]);
    let rec = run(&s, chat(&s, false)).await;
    let p = &phases::of(&rec)[0];
    assert_eq!(p.value(Phase::Generation), PhaseValue::NotApplicable);
    let t = rec.attempts[0].timing.as_ref().unwrap();
    assert_eq!(t.first_output, t.upstream_done);
    sum_matches(&rec);
}

#[tokio::test]
async fn a_client_that_leaves_mid_generation_ends_the_attempt_in_generation() {
    let s = server().await;
    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::from_millis(300), frames(0))]);
    let r = chat(&s, true).send().await.unwrap();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let mut body = r.bytes_stream();
    body.next().await;
    drop(body);
    let rec = finished(&s, &id).await;
    let all = phases::of(&rec);
    assert_eq!(all[0].ended_in, Some(Phase::Generation));
    assert_eq!(all[0].value(Phase::Delivery), PhaseValue::NotApplicable);
}

/// T015: once a request ends, its live entry is gone, whether it was served, failed or the
/// client left.
#[tokio::test]
async fn a_finished_request_leaves_no_live_entry() {
    let s = server().await;
    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::ZERO, frames(0))]);
    let rec = run(&s, chat(&s, true)).await;
    assert!(rec.total_ms.is_some());
    gone(&s).await;

    s.mock.push([phased(Duration::ZERO, Duration::ZERO, Duration::from_millis(300), frames(0))]);
    let r = chat(&s, true).send().await.unwrap();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let mut body = r.bytes_stream();
    body.next().await;
    drop(body);
    finished(&s, &id).await;
    gone(&s).await;
}

/// The table is empty within a second of the last request ending (SC-005).
async fn gone(s: &common::Server) {
    for _ in 0..200 {
        if s.engine.live.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{} live entries left", s.engine.live.len());
}

#[test]
fn a_record_from_before_the_slice_reads_as_not_recorded() {
    let mut old = json!({
        "id": "rq_old", "outcome": "succeeded", "ttft_ms": 10.0, "total_ms": 20.0,
        "attempts": [{"n": 1, "kind": "initial", "started": 1.0, "ended": 19.0}]
    });
    phases::decorate(&mut old, None);
    assert_eq!(old["attempts"][0]["phases"], "not_recorded");
    assert!(old["slowest"].is_null());
}

/// SC-003: the request-level router overhead is the first non-skipped attempt's `started`, and
/// request TTFT is `ttft_ms` (slice 010's definitions, re-pointed to `phases::of`).
// 010: router overhead = the first attempt's `started`, less a token refresh; TTFT = `ttft_ms`.
#[tokio::test]
async fn router_overhead_and_ttft_keep_slice_010s_definitions() {
    let s = server().await;
    s.mock.push([phased(Duration::ZERO, Duration::from_millis(50), EVERY, frames(0))]);
    let rec = run(&s, chat(&s, true)).await;
    let first = rec.attempts.iter().find(|a| a.kind != AttemptKind::Skipped).unwrap();
    let refresh = first.timing.as_ref().and_then(|t| t.refresh_ms).unwrap_or(0.0);
    let p = &phases::of(&rec)[0];
    assert!((ms(p, Phase::RouterOverhead) - (first.started - refresh)).abs() < 0.01);
    ttft_matches(&rec);
}
