//! A client that goes away while a third-party adapter is on its path (spec 004, US3, T068).
//!
//! Two cases. With the adapter on a stream, the provider sees its connection close within a
//! second and the record says `cancelled`. With the adapter spinning in its request call (the
//! request deadline raised to 30 s through the test-only override, since the real one is a
//! constant), the call is dropped with the attempt: the provider is never asked, the record says
//! `cancelled` rather than `failed{deadline}`, and no sandbox instance is left.
//!
//! In the first case the adapter spins in `zr_on_event`, but the event deadline (2 ms) ends the
//! spin as `failed{deadline}` before the drop; that case covers the adapter being on the path.

mod common;

use std::time::{Duration, Instant};

use common::server;
use nullrouter_adapters::testkit::{Behaviour, install_fixture, wat_adapter};
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::{Outcome, Query};
use nullrouter_engine::testkit::Step;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::json;

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

fn spinning_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
            (func (export "zr_on_response") (param i32 i32) (result i64) i64.const 0)
            (func (export "zr_on_event") (param i32 i32) (result i64)
              (loop $l (br $l))
              i64.const 0)
            (@custom "nr.abi" "\01\00\00\00"))"#,
    )
    .unwrap()
}

#[tokio::test]
async fn a_client_drop_with_an_adapter_on_the_stream_closes_the_upstream_within_a_second() {
    let s = server().await;
    install_fixture(s.home(), "acme", MANIFEST, &spinning_module());
    s.engine.open_adapters().unwrap();
    let mut keys = Keys::load(&s.home().join("keys.toml")).unwrap();
    keys.set_adapter("laptop", Some(HarnessName::new("acme").unwrap())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();

    let first = format!("data: {}\n\n", json!({"id": "up-1", "choices": [{"index": 0, "delta": {"content": "Hel"}}]}));
    s.mock.push([Step::StallAfter { frames: vec![first.into()], hold: Duration::from_secs(30) }]);
    let mut r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(
            json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]}).to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let chunk = r.chunk().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&chunk).contains("data: "), "the first bytes arrived before the drop");

    drop(r);
    let dropped = Instant::now();
    while dropped.elapsed() < Duration::from_secs(1) {
        if !s.mock.disconnects().is_empty() && s.engine.records.get(&id).unwrap().outcome == Outcome::Cancelled {
            assert_eq!(s.engine.adapter_live_instances(), 0, "no sandbox instance outlives the stream");
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

const REQUEST_ONLY: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["model"]
"#;

#[tokio::test]
async fn a_client_drop_during_a_spinning_request_call_drops_the_call_and_never_reaches_the_provider() {
    let s = server().await;
    install_fixture(s.home(), "acme", REQUEST_ONLY, &wat_adapter(Behaviour::Loop));
    s.engine.open_adapters().unwrap();
    s.engine.override_adapter_deadlines(Duration::from_secs(30), Duration::from_secs(30));
    let mut keys = Keys::load(&s.home().join("keys.toml")).unwrap();
    keys.set_adapter("laptop", Some(HarnessName::new("acme").unwrap())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();

    let sent = tokio::spawn(
        reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", s.base))
            .bearer_auth(&s.key)
            .header("content-type", "application/json")
            .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
            .send(),
    );
    // Drop the client only once the call holds its sandbox instance, so the test can't pass
    // before the adapter ever ran.
    let started = Instant::now();
    while s.engine.adapter_live_instances() == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "the adapter call never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    sent.abort();

    let dropped = Instant::now();
    while dropped.elapsed() < Duration::from_secs(1) {
        let records = s.engine.records.query(&Query::default());
        // The one record says cancelled, never `failed{deadline}` or a success.
        let ended = records.len() == 1 && records[0].outcome == Outcome::Cancelled;
        if ended && s.engine.adapter_live_instances() == 0 {
            assert!(s.mock.received().is_empty(), "the provider was never asked");
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "after 1 s: live instances {}; outcomes {:?}; upstream requests {}",
        s.engine.adapter_live_instances(),
        s.engine.records.query(&Query::default()).iter().map(|r| r.outcome).collect::<Vec<_>>(),
        s.mock.received().len()
    );
}
