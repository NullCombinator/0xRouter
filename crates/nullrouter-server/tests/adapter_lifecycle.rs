//! The adapter lifecycle under load (spec 004, US4 scenarios 1 to 3, SC-006, T072): while 20
//! streams run on `v1`, a second version goes through `in_review`, `quarantined`, `reported`,
//! `rejected`, and then a resubmission is approved. No request fails, every request that
//! started before the approval is served by `v1`, every one that starts after it by `v2`, and
//! the streams in flight finish on `v1`.
//!
//! Versions are driven at store level: no builder or reviewer runs. The testkit puts a new
//! version straight to `in_review`, so `queued` and `building` are not observed here.

mod common;

use std::time::Duration;

use common::{Server, server};
use nullrouter_adapters::record::AdapterOutcome;
use nullrouter_adapters::store::{Store, VersionId, VersionState};
use nullrouter_adapters::testkit::{install_fixture, install_in_review};
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::{Outcome, RequestRecord};
use nullrouter_engine::testkit::Step;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["model"]

[response]
selectors = ["choices[*].delta"]
events = true
"#;
const EDIT_AT: i64 = 32768;
const CLIENTS: usize = 20;
/// Time between the frames of a slow stream; it keeps the streams in flight while the operator
/// works.
const EVERY: Duration = Duration::from_millis(800);

/// `text` as the body of a WAT data string.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b' '..=b'~' if b != b'"' && b != b'\\' => (b as char).to_string(),
            _ => format!("\\{b:02x}"),
        })
        .collect()
}

/// A module that replaces the content of every event whose text ends in a digit with `mark`.
/// It reads the last character of the text (the input ends `x"}}]}` for a delta
/// `{"content":"x"}`); the opening and finishing chunks are left alone.
fn module(mark: &str) -> Vec<u8> {
    let edit = json!({"edits": [{"op": "replace", "path": "choices[0].delta.content", "kind": "converted",
        "reason": "format_conversion", "value": mark}]})
    .to_string();
    let packed = (EDIT_AT << 32) | i64::try_from(edit.len()).unwrap();
    wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const {EDIT_AT}) "{}")
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
            (func (export "zr_on_response") (param i32 i32) (result i64) i64.const 0)
            (func (export "zr_on_event") (param $p i32) (param $n i32) (result i64)
              (local $c i32)
              (local.set $c (i32.load8_u (i32.add (local.get $p) (i32.sub (local.get $n) (i32.const 6)))))
              (if (i32.and (i32.ge_u (local.get $c) (i32.const 48)) (i32.le_u (local.get $c) (i32.const 57)))
                (then (return (i64.const {packed}))))
              i64.const 0)
            (@custom "nr.abi" "\01\00\00\00"))"#,
        escape(&edit),
    ))
    .unwrap()
}

/// The upstream's answer: four deltas of `0`, a finish chunk and `[DONE]`. A request whose body
/// mentions `slow` gets its frames `EVERY` apart.
fn answer(slow: bool) -> Step {
    let chunk = |delta: Value, finish: Value| {
        (
            None,
            json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1",
                   "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
        )
    };
    let mut events: Vec<_> = (0..4).map(|_| chunk(json!({"content": "0"}), Value::Null)).collect();
    events.push(chunk(json!({}), json!("stop")));
    let step = Step::sse(&events, true);
    match step {
        Step::Stream { status, headers, frames, cut, .. } if slow => {
            Step::Stream { status, headers, frames, every: EVERY, cut }
        }
        other => other,
    }
}

fn name() -> HarnessName {
    HarnessName::new("acme").unwrap()
}

/// Binds the server's key to `acme` and reloads.
fn bind(s: &Server) {
    let mut keys = Keys::load(&s.home().join("keys.toml")).unwrap();
    keys.set_adapter("laptop", Some(name())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
}

/// One streaming request. With `first`, a message is sent once the first bytes have arrived.
/// Returns the request id and the whole body.
async fn ask(
    base: String,
    key: String,
    prompt: &str,
    mut first: Option<tokio::sync::mpsc::Sender<()>>,
) -> (String, String) {
    let mut r = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(key)
        .header("content-type", "application/json")
        .body(
            json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": prompt}]})
                .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let mut text = String::new();
    while let Some(chunk) = r.chunk().await.unwrap() {
        text.push_str(&String::from_utf8_lossy(&chunk));
        if let Some(tx) = first.as_ref().filter(|_| text.contains("data: ")) {
            // Only the first bytes announce themselves.
            tx.send(()).await.unwrap();
            first = None;
        }
    }
    (id, text)
}

async fn settled(s: &Server, id: &str) -> RequestRecord {
    for _ in 0..400 {
        if let Some(rec) = s.engine.records.get(id).filter(|r| r.total_ms.is_some()) {
            return rec;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the record never settled: {:?}", s.engine.records.get(id));
}

fn content(text: &str) -> String {
    text.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .filter_map(|v| v["choices"][0]["delta"]["content"].as_str().map(str::to_owned))
        .collect()
}

/// A finished request served by `version`: success, the adapter ran, `expect` is the content.
async fn assert_served(s: &Server, (id, text): &(String, String), version: &VersionId, expect: &str) {
    assert_eq!(content(text), expect, "{text}");
    let rec = settled(s, id).await;
    assert_eq!(rec.outcome, Outcome::Succeeded, "{rec:?}");
    let run = rec.response_adapter.expect("the stream's adapter run is recorded");
    assert_eq!(run.outcome, AdapterOutcome::Ran, "{run:?}");
    assert_eq!(run.version, version.as_str(), "{run:?}");
}

/// One edge of the state machine, written to the index; the engine then reads the store again.
fn move_to(s: &Server, id: &VersionId, to: VersionState) {
    let store = Store::open(s.home()).unwrap();
    let mut index = store.load_index().unwrap();
    index.transition(&name(), id, to, "").unwrap();
    store.save_index(&index).unwrap();
    s.engine.refresh_adapters();
}

/// A request that starts now, run to its end, served by `version`.
async fn probe(s: &Server, version: &VersionId, expect: &str) {
    let got = ask(s.base.clone(), s.key.clone(), "fast", None).await;
    assert_served(s, &got, version, expect).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streams_in_flight_finish_on_the_old_version_and_new_requests_run_the_approved_one() {
    let s = server().await;
    s.mock.respond(|r| answer(String::from_utf8_lossy(&r.body).contains("slow")));
    let v1 = install_fixture(s.home(), "acme", MANIFEST, &module("1"));
    s.engine.open_adapters().unwrap();
    bind(&s);
    probe(&s, &v1, "1111").await;

    // 20 streams start on v1 and are held mid-stream by the slow upstream.
    let (tx, mut rx) = tokio::sync::mpsc::channel(CLIENTS * 2);
    let clients: Vec<_> =
        (0..CLIENTS).map(|_| tokio::spawn(ask(s.base.clone(), s.key.clone(), "slow", Some(tx.clone())))).collect();
    for _ in 0..CLIENTS {
        rx.recv().await.expect("every client gets its first bytes");
    }

    // The first v2 is reviewed, quarantined, reviewed again and rejected. v1 serves throughout.
    let v2 = install_in_review(s.home(), "acme", MANIFEST, &module("2"), (2, 0, 0), "// first v2");
    probe(&s, &v1, "1111").await;
    move_to(&s, &v2, VersionState::Quarantined);
    probe(&s, &v1, "1111").await;
    move_to(&s, &v2, VersionState::InReview);
    move_to(&s, &v2, VersionState::Reported);
    probe(&s, &v1, "1111").await;
    nullrouter_adapters::reject(s.home(), &name(), &v2, Some("not this one")).unwrap();
    s.engine.refresh_adapters();
    probe(&s, &v1, "1111").await;

    // The resubmission reaches `reported` and is approved while the 20 streams are still going.
    let v2b = install_in_review(s.home(), "acme", MANIFEST, &module("2"), (2, 0, 1), "// second v2");
    move_to(&s, &v2b, VersionState::Reported);
    probe(&s, &v1, "1111").await;
    assert!(clients.iter().all(|c| !c.is_finished()), "the approval has to come while every stream is in flight");
    nullrouter_adapters::approve(s.home(), &name(), &v2b, None).unwrap();
    s.engine.refresh_adapters();

    // Requests that start after the approval run v2, 20 at a time, while the old streams finish.
    let after: Vec<_> = (0..CLIENTS).map(|_| tokio::spawn(ask(s.base.clone(), s.key.clone(), "fast", None))).collect();
    for c in after {
        assert_served(&s, &c.await.unwrap(), &v2b, "2222").await;
    }
    for c in clients {
        assert_served(&s, &c.await.unwrap(), &v1, "1111").await;
    }

    let index = Store::open(s.home()).unwrap().load_index().unwrap();
    assert_eq!(index.harness(&name()).and_then(|h| h.active.clone()), Some(v2b.clone()));
    assert_eq!(index.version(&name(), &v1).unwrap().state, VersionState::Superseded);
    assert_eq!(index.version(&name(), &v2).unwrap().state, VersionState::Rejected);
    assert_eq!(index.version(&name(), &v2b).unwrap().state, VersionState::Approved);
    assert_eq!(s.engine.adapter_live_instances(), 0);
}
