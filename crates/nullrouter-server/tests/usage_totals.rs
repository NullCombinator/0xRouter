//! The operator socket's `usage.totals` op (spec 010, US1, T013): the server's answer equals the
//! cold read of the same disk, a request still in the live ring is counted once, and a finished
//! day served from the cache is recomputed when its file changes.

use std::path::Path;
use std::sync::Arc;

use nullrouter_engine::accounts::{self, Accounts};
use nullrouter_engine::clock::parse_rfc3339;
use nullrouter_engine::journal::summary::{self, Window};
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use serde_json::{Value, json};

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

async fn totals(engine: &Arc<Engine>, from: Option<&str>) -> Value {
    let a = operator::handle(engine, &json!({"op": "usage.totals", "from": from, "to": TO})).await;
    assert_eq!(a["ok"], true, "{a}");
    a["totals"].clone()
}

/// The cold read of the same disk, as the CLI without a server would make it.
fn cold(engine: &Engine, home: &Path, from: Option<&str>) -> Value {
    let st = engine.snapshot();
    let accounts = Accounts::load(&home.join(accounts::FILE)).unwrap();
    let prices = summary::prices_of(&st.registry, &accounts);
    let w = Window { from: from.map(|f| parse_rfc3339(f).unwrap()), to: parse_rfc3339(TO).unwrap() };
    serde_json::to_value(summary::totals(home, &w, &prices, true)).unwrap()
}

fn append(path: &Path, id: &str, arrived: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(f, r#"{{"v":1,"t":"open","id":"{id}","arrived":"{arrived}","agent":"ak_new","style":"anthropic-messages","op":"generate","type":"text","target":"t"}}"#).unwrap();
    writeln!(f, r#"{{"v":1,"t":"close","id":"{id}","outcome":"succeeded","served_by":null,"ttft_ms":1.0,"total_ms":2.0,"usage":null,"break_handling":{{"kind":"none"}},"job":null}}"#).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_answers_what_the_cold_read_gives() {
    let dir = fixture_home();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    for from in [None, Some("2026-10-06T00:00:00Z"), Some("2026-10-06T12:00:00Z")] {
        assert_eq!(totals(&engine, from).await, cold(&engine, dir.path(), from), "from {from:?}");
    }
    assert_eq!(totals(&engine, None).await["requests"], 15);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_in_the_live_ring_counts_once_and_replaces_its_disk_copy() {
    let dir = fixture_home();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    let base = totals(&engine, None).await;

    // Not on disk: one more request, in flight.
    engine.records.insert(RequestRecord::new("rq_live".into(), "2026-10-07T11:59:00Z".into(), "anthropic-messages"));
    let with = totals(&engine, None).await;
    assert_eq!(with["requests"], base["requests"].as_u64().unwrap() + 1);
    assert_eq!(with["in_flight"], base["in_flight"].as_u64().unwrap() + 1);
    assert_eq!(totals(&engine, None).await, with, "asking again changes nothing");

    // On disk already (rq_fx15 is in the journal, unfinished): the ring copy replaces it.
    engine.records.insert(RequestRecord::new("rq_fx15".into(), "2026-10-07T11:55:00Z".into(), "anthropic-messages"));
    assert_eq!(totals(&engine, None).await["requests"], with["requests"], "not counted twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cached_day_is_recomputed_when_its_file_changes() {
    let dir = fixture_home();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    let first = totals(&engine, None).await;
    assert_eq!(totals(&engine, None).await, first, "served from the cache");

    // The file grows: the length changes.
    let day = dir.path().join("records/2026-10-05.jsonl");
    append(&day, "rq_extra", "2026-10-05T12:00:00Z");
    assert_eq!(totals(&engine, None).await["requests"], first["requests"].as_u64().unwrap() + 1);

    // The file is replaced (as `records forget` does): a new inode, fewer records.
    let kept: String = std::fs::read_to_string(&day).unwrap().lines().take(3).map(|l| l.to_owned() + "\n").collect();
    let swap = dir.path().join("records/swap.tmp");
    std::fs::write(&swap, kept).unwrap();
    std::fs::rename(&swap, &day).unwrap();
    assert_eq!(totals(&engine, None).await, cold(&engine, dir.path(), None));

    // A reload is a new generation: the cache is not trusted across it.
    engine.reload().await.unwrap();
    assert_eq!(totals(&engine, None).await, cold(&engine, dir.path(), None));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_that_is_not_a_time_is_refused() {
    let dir = fixture_home();
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    let engine = Arc::new(engine);
    let a = operator::handle(&engine, &json!({"op": "usage.totals", "from": "yesterday", "to": TO})).await;
    assert_eq!(a["ok"], false);
    let a = operator::handle(&engine, &json!({"op": "usage.totals", "from": null, "to": null})).await;
    assert_eq!(a["ok"], false);
}
