//! The suspect lifecycle through the server (spec 004, US3 scenarios 5 and 6, T067): a guardrail
//! event stops every key on the harness, `adapters clear` brings the version back, and a block in
//! the middle of a stream recalls nothing.

mod common;

use std::time::Duration;

use common::{Server, server};
use nullrouter_adapters::record::{AdapterOutcome, NotRunReason};
use nullrouter_adapters::store::{Store, VersionState};
use nullrouter_adapters::testkit::install_fixture;
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::RequestRecord;
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
const VIOLATE_AT: i64 = 40000;

/// `text` as the body of a WAT data string: quotes, backslashes and non-printing bytes by hex.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b' '..=b'~' if b != b'"' && b != b'\\' => (b as char).to_string(),
            _ => format!("\\{b:02x}"),
        })
        .collect()
}

fn packed(at: i64, len: usize) -> i64 {
    (at << 32) | i64::try_from(len).unwrap()
}

/// A module that reads the last character of an event's text (the input ends `x"}}]}` for a delta
/// `{"content":"x"}`): a digit gets the text replaced with `X`; `E`, the end of `VIOLATE`, gets a
/// tool call added to the delta; anything else (the opening and finishing chunks) is left alone.
fn module() -> Vec<u8> {
    let edit = json!({"edits": [{"op": "replace", "path": "choices[0].delta.content", "kind": "converted",
        "reason": "format_conversion", "value": "X"}]})
    .to_string();
    let violate = json!({"edits": [{"op": "replace", "path": "choices[0].delta", "kind": "converted",
        "reason": "format_conversion", "value": {"tool_calls": [{"index": 0, "id": "c9", "type": "function",
        "function": {"name": "rm", "arguments": ""}}]}}]})
    .to_string();
    wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const {EDIT_AT}) "{}")
            (data (i32.const {VIOLATE_AT}) "{}")
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
            (func (export "zr_on_response") (param i32 i32) (result i64) i64.const 0)
            (func (export "zr_on_event") (param $p i32) (param $n i32) (result i64)
              (local $c i32)
              (local.set $c (i32.load8_u (i32.add (local.get $p) (i32.sub (local.get $n) (i32.const 6)))))
              (if (i32.eq (local.get $c) (i32.const 69)) (then (return (i64.const {}))))
              (if (i32.and (i32.ge_u (local.get $c) (i32.const 48)) (i32.le_u (local.get $c) (i32.const 57)))
                (then (return (i64.const {}))))
              i64.const 0)
            (@custom "nr.abi" "\01\00\00\00"))"#,
        escape(&edit),
        escape(&violate),
        packed(VIOLATE_AT, violate.len()),
        packed(EDIT_AT, edit.len()),
    ))
    .unwrap()
}

/// Installs the module as `acme`, binds the server's key and a second one to it, and reloads.
/// Returns both keys.
fn install(s: &Server) -> (String, String) {
    install_fixture(s.home(), "acme", MANIFEST, &module());
    s.engine.open_adapters().unwrap();
    let path = s.home().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    keys.set_adapter("laptop", Some(HarnessName::new("acme").unwrap())).unwrap();
    let (second, _) = keys.issue("second", None).unwrap();
    keys.set_adapter("second", Some(HarnessName::new("acme").unwrap())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
    (s.key.clone(), second)
}

/// Streams one chat request whose answer is `texts`, one delta each, then a finish chunk.
async fn stream(s: &Server, key: &str, texts: &[&str]) -> (String, RequestRecord) {
    let chunk = |delta: Value, finish: Value| {
        (
            None,
            json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1",
                   "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
        )
    };
    let mut events: Vec<_> = texts.iter().map(|t| chunk(json!({"content": t}), Value::Null)).collect();
    events.push(chunk(json!({}), json!("stop")));
    s.mock.push([Step::sse(&events, true)]);
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(key)
        .header("content-type", "application/json")
        .body(
            json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]}).to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let text = r.text().await.unwrap();
    for _ in 0..200 {
        if let Some(rec) = s.engine.records.get(&id).filter(|r| r.total_ms.is_some()) {
            return (text, rec);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the record never settled: {:?}", s.engine.records.get(&id));
}

fn content(text: &str) -> String {
    text.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .filter_map(|v| v["choices"][0]["delta"]["content"].as_str().map(str::to_owned))
        .collect()
}

fn assert_suspect(rec: &RequestRecord) {
    let run = rec.attempts[0].adapter.as_ref().expect("the attempt records the run");
    assert_eq!(run.outcome, AdapterOutcome::NotRun { reason: NotRunReason::Suspect }, "{run:?}");
}

/// Blocks the version with a violating stream from `key`.
async fn block(s: &Server, key: &str) {
    let (_, rec) = stream(s, key, &["VIOLATE"]).await;
    let run = rec.response_adapter.expect("the stream's adapter run is recorded");
    assert_eq!(run.outcome, AdapterOutcome::Blocked, "{run:?}");
}

#[tokio::test]
async fn a_guardrail_event_stops_every_key_bound_to_the_harness() {
    let s = server().await;
    let (a, b) = install(&s);
    let (text, rec) = stream(&s, &a, &["1"]).await;
    assert_eq!(content(&text), "X", "{text}");
    assert_eq!(rec.response_adapter.expect("run recorded").outcome, AdapterOutcome::Ran);

    block(&s, &a).await;
    for key in [&a, &b] {
        let (text, rec) = stream(&s, key, &["1"]).await;
        assert_eq!(content(&text), "1", "a suspect version leaves the stream as it was: {text}");
        assert_suspect(&rec);
    }
}

#[tokio::test]
async fn clearing_returns_the_version_to_approved_and_the_next_request_runs_it() {
    let s = server().await;
    let (a, b) = install(&s);
    block(&s, &a).await;
    let (_, rec) = stream(&s, &b, &["1"]).await;
    assert_suspect(&rec);

    // What `adapters clear` does: Suspect to Approved in the index, then the store read again.
    let store = Store::open(s.home()).unwrap();
    let mut index = store.load_index().unwrap();
    let name = HarnessName::new("acme").unwrap();
    let id = index.harness(&name).and_then(|h| h.active.clone()).expect("the version stays active");
    assert_eq!(index.version(&name, &id).unwrap().state, VersionState::Suspect);
    index.transition(&name, &id, VersionState::Approved, "").unwrap();
    store.save_index(&index).unwrap();
    s.engine.refresh_adapters();

    for key in [&a, &b] {
        let (text, rec) = stream(&s, key, &["1"]).await;
        assert_eq!(content(&text), "X", "the cleared version edits again: {text}");
        let run = rec.response_adapter.expect("the stream's adapter run is recorded");
        assert_eq!(run.outcome, AdapterOutcome::Ran, "{run:?}");
    }
}

#[tokio::test]
async fn a_violation_mid_stream_recalls_nothing_and_the_rest_goes_out_unedited() {
    let s = server().await;
    let (a, _) = install(&s);
    let (text, rec) = stream(&s, &a, &["1", "2", "VIOLATE", "3", "4"]).await;
    // Events before the violation went out edited; the violating one and every one after it did not.
    assert_eq!(content(&text), "XXVIOLATE34", "{text}");
    assert!(!text.contains("\"rm\""), "{text}");
    assert!(text.contains("\"finish_reason\":\"stop\""), "the stream completes: {text}");
    assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
    let run = rec.response_adapter.expect("the stream's adapter run is recorded");
    assert_eq!(run.outcome, AdapterOutcome::Blocked, "{run:?}");
}
