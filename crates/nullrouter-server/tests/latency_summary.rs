//! The operator socket's `latency.summary` op (spec 010, US2, T023): the server's answer equals
//! the cold read of the same disk, and a request in the live ring replaces its disk copy.

use std::path::Path;
use std::sync::Arc;

use nullrouter_engine::clock::parse_rfc3339;
use nullrouter_engine::journal::summary::{self, Window};
use nullrouter_engine::keys::AgentId;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use serde_json::{Value, json};

const FROM: &str = "2026-10-06T12:00:00Z";
const TO: &str = "2026-10-07T12:00:00Z";

fn fixture_home() -> tempfile::TempDir {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../nullrouter-engine/tests/fixtures/summaries");
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("records")).unwrap();
    for entry in std::fs::read_dir(from.join("records")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), home.path().join("records").join(entry.file_name())).unwrap();
    }
    home
}

async fn latency(engine: &Arc<Engine>) -> Value {
    let a = operator::handle(engine, &json!({"op": "latency.summary", "from": FROM, "to": TO})).await;
    assert_eq!(a["ok"], true, "{a}");
    a["latency"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_answers_what_the_cold_read_gives() {
    let dir = fixture_home();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    let w = Window { from: Some(parse_rfc3339(FROM).unwrap()), to: parse_rfc3339(TO).unwrap() };
    let cold = serde_json::to_value(summary::latency(dir.path(), &w, &[], true)).unwrap();
    let served = latency(&engine).await;
    assert_eq!(served, cold);
    assert_eq!(served["agents"]["ak_alice"]["requests"], 5);
    assert_eq!(served["providers"]["alpha"]["requests"], 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_in_the_live_ring_counts_once_and_replaces_its_disk_copy() {
    let dir = fixture_home();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    let base = latency(&engine).await["agents"]["ak_bob"]["requests"].as_u64().unwrap();

    let mut live = RequestRecord::new("rq_live".into(), "2026-10-07T11:59:00Z".into(), "anthropic-messages");
    live.agent = Some(AgentId::new("ak_bob", None));
    engine.records.insert(live);
    assert_eq!(latency(&engine).await["agents"]["ak_bob"]["requests"], base + 1);
    assert_eq!(latency(&engine).await["agents"]["ak_bob"]["requests"], base + 1, "asking again changes nothing");

    // rq_fx15 is on disk (ak_bob, unfinished); the ring copy has no agent, so it leaves ak_bob's row.
    engine.records.insert(RequestRecord::new("rq_fx15".into(), "2026-10-07T11:55:00Z".into(), "anthropic-messages"));
    assert_eq!(latency(&engine).await["agents"]["ak_bob"]["requests"], base, "replaced, not added");
}
