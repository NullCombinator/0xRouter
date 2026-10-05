//! The `close` line is on disk before the client's last byte (spec 006, US5, T063, FR-037).
//!
//! With the writer's ack held by the fault hook, the line is written but not acknowledged: a
//! streaming client gets the answer up to its ending and not the ending, and a non-streaming
//! client gets no body. Once the ack is released the rest arrives, and the record is on disk.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{chat_stream, chat_whole, server};
use nullrouter_engine::journal::records;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

/// Holds acks, letting each request's `open` mark through (it waits for it before the first
/// upstream call) so only its `close` is held.
fn pause(s: &common::Server, on: bool) {
    let f = s.engine.journal.faults();
    f.free_marks.store(usize::from(on), Ordering::Relaxed);
    f.pause_acks.store(on, Ordering::Relaxed);
}

/// The record `id` as the journal holds it, read off the executor.
async fn on_disk(s: &common::Server, id: &str) -> Option<Value> {
    let (home, id) = (s.home().to_owned(), id.to_owned());
    tokio::task::spawn_blocking(move || records::get(&home, &id)).await.unwrap()
}

#[tokio::test]
async fn a_whole_answer_waits_for_its_close_line() {
    let s = server().await;
    s.mock.push([chat_whole()]);
    pause(&s, true);
    let url = format!("{}/v1/chat/completions", s.base);
    let key = s.key.clone();
    let sent = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .bearer_auth(key)
            .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
            .send()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!sent.is_finished(), "no part of the answer reaches the client while the close is unacknowledged");
    // The line itself is already written: only the ack is held.
    let found = tokio::task::spawn_blocking({
        let home = s.home().to_owned();
        move || records::read(&home, &records::Filter::default())
    })
    .await
    .unwrap();
    assert!(found.iter().any(|r| r["outcome"] == "succeeded"), "the close line is in the file: {found:?}");

    pause(&s, false);
    let r = tokio::time::timeout(Duration::from_secs(5), sent).await.expect("released").unwrap();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    assert_eq!(r.status(), 200);
    let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "hi there");
    assert_eq!(on_disk(&s, &id).await.expect("on disk")["outcome"], "succeeded");
}

#[tokio::test]
async fn a_stream_is_relayed_up_to_its_ending_and_the_ending_waits() {
    let s = server().await;
    s.mock.push([chat_stream()]);
    pause(&s, true);
    let mut r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(
            json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]}).to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();

    // Everything before the ending comes through while the ack is held.
    let mut seen = String::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(Duration::from_millis(400), r.chunk()).await {
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.contains("Hel"), "the content streamed without waiting: {seen}");
    assert!(!seen.contains("[DONE]"), "the final event waits for the close ack: {seen}");

    pause(&s, false);
    let mut rest = String::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout(Duration::from_secs(5), r.chunk()).await {
        rest.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(rest.contains("[DONE]"), "released: {rest}");
    assert_eq!(on_disk(&s, &id).await.expect("on disk")["outcome"], "succeeded");
}
