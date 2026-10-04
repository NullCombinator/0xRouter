//! Warm fingerprints and deficits survive a restart (spec 006, US5, T061): an agent warm on an
//! account before goes to it after, deficits continue, expired fingerprints and past windows are
//! left behind, and compaction keeps exactly what is live.

mod common;

use std::sync::Arc;
use std::time::SystemTime;

use common::*;
use nullrouter_engine::journal::state;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::route;
use nullrouter_engine::routing::PlacementReason;
use nullrouter_engine::routing::view::TargetView;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const SUB: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "5m"
min_tokens = 0

[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
"#;

const SUB_SHORT_CACHE: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "1s"
min_tokens = 0

[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
"#;

fn fleet(routing: &str) -> Fleet {
    Fleet::new()
        .provider("alpha", routing, &[("5h", "tokens")])
        .account("alpha", "one", 1.0)
        .account("alpha", "two", 1.0)
        .unified(&[("alpha", "m1")])
}

fn body(i: usize) -> Value {
    json!({"model": "u", "stream": false, "messages": [
        {"role": "system", "content": "You are a careful assistant."},
        {"role": "user", "content": format!("question {i} {}", "word ".repeat(200))},
    ]})
}

async fn turn(f: &FleetSetup, agent: &str, i: usize) -> RequestRecord {
    let req = request(&f.setup, "openai-chat", "u", body(i), agent, CancellationToken::new());
    let id = req.id.clone();
    f.setup.engine.text(f.setup.engine.snapshot(), req).await.expect("served");
    settled(&f.setup, &id).await
}

/// A stop and start over the same home: the writer finishes what it has, a new engine loads it.
async fn restart(f: &mut FleetSetup) {
    let old = f.setup.engine.clone();
    tokio::task::spawn_blocking(move || old.journal.shutdown()).await.unwrap();
    let (engine, _) = Engine::open_parity(OperatorHome::new(f.setup._dir.path())).unwrap();
    f.setup.engine = Arc::new(engine);
}

/// Stops the writer so the test can change the files, then starts again.
async fn restart_after(f: &mut FleetSetup, edit: impl FnOnce(&std::path::Path)) {
    let old = f.setup.engine.clone();
    tokio::task::spawn_blocking(move || old.journal.shutdown()).await.unwrap();
    edit(f.setup._dir.path());
    let (engine, _) = Engine::open_parity(OperatorHome::new(f.setup._dir.path())).unwrap();
    f.setup.engine = Arc::new(engine);
}

fn view(f: &FleetSetup) -> TargetView {
    let st = f.setup.engine.snapshot();
    route::view_all(&f.setup.engine, &st, Some("u"), SystemTime::now()).remove(0)
}

fn deficits(v: &TargetView) -> Vec<(String, i64)> {
    v.accounts.iter().map(|a| (a.account.clone(), a.deficit)).collect()
}

fn served(r: &RequestRecord) -> String {
    let s = r.served_by.as_ref().expect("served");
    format!("{}/{}", s.provider, s.account.as_deref().unwrap_or(""))
}

fn lines(home: &std::path::Path, file: &str) -> Vec<Value> {
    std::fs::read_to_string(home.join("routing").join(file))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn an_agent_warm_on_an_account_goes_back_to_it_after_a_restart() {
    let mut f = fleet(SUB).build().await;
    // Make the second account the one with the larger deficit, so a cold request would go to it.
    let first = turn(&f, "ak_a", 0).await;
    let home = served(&first);
    let before = f.setup.engine.router.lock().warm.len();
    assert!(before > 0, "the request left fingerprints");

    restart(&mut f).await;
    assert_eq!(f.setup.engine.router.lock().warm.len(), before, "the fingerprints came back");

    let again = turn(&f, "ak_a", 0).await;
    assert_eq!(served(&again), home, "the warm agent stays on its account");
    assert_eq!(again.attempts.last().unwrap().placement.unwrap().reason, PlacementReason::Warm);
}

#[tokio::test]
async fn deficits_after_a_restart_equal_those_before() {
    let mut f = fleet(SUB).build().await;
    for i in 0..5 {
        turn(&f, &format!("ak_{i}"), i).await;
    }
    let before = deficits(&view(&f));
    assert!(before.iter().any(|(_, d)| *d != 0), "{before:?}");

    restart(&mut f).await;
    assert_eq!(deficits(&view(&f)), before);
}

#[tokio::test]
async fn fingerprints_past_their_lifetime_are_dropped_at_load() {
    let mut f = fleet(SUB_SHORT_CACHE).build().await;
    turn(&f, "ak_a", 0).await;
    assert!(!f.setup.engine.router.lock().warm.is_empty());
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

    restart(&mut f).await;
    assert_eq!(f.setup.engine.router.lock().warm.len(), 0);
    let engine = f.setup.engine.clone();
    tokio::task::spawn_blocking(move || engine.journal.flush_blocking()).await.unwrap();
    assert!(lines(f.setup._dir.path(), "warm.jsonl").is_empty(), "and the file no longer holds them");
}

#[tokio::test]
async fn ledger_lines_from_a_past_window_are_ignored() {
    let mut f = fleet(SUB).build().await;
    turn(&f, "ak_a", 0).await;
    restart_after(&mut f, |home| {
        let past = state::ledger_line(
            "u",
            nullrouter_engine::routing::Tier::Subscription,
            nullrouter_engine::clock::parse_rfc3339("2020-01-01T00:00:00Z").unwrap(),
            &[("alpha/one".to_owned(), 5000.0), ("alpha/two".to_owned(), -5000.0)].into_iter().collect(),
            SystemTime::now(),
        );
        std::fs::write(home.join("routing/ledger.jsonl"), past.to_string() + "\n").unwrap();
    })
    .await;
    assert!(deficits(&view(&f)).iter().all(|(_, d)| *d == 0), "{:?}", deficits(&view(&f)));
}

#[tokio::test]
async fn compaction_keeps_exactly_the_live_state() {
    let mut f = fleet(SUB).build().await;
    for i in 0..4 {
        turn(&f, "ak_a", i).await;
    }
    let live: Vec<_> = f.setup.engine.router.lock().warm.stored();
    // The files grew by every request; add a stale duplicate and a removed account's fingerprint.
    restart_after(&mut f, |home| {
        let mut dup = live[0].clone();
        dup.last_used -= std::time::Duration::from_secs(30);
        let mut gone = live[0].clone();
        gone.at.account = "removed".into();
        let mut warm = std::fs::read_to_string(home.join("routing/warm.jsonl")).unwrap();
        warm += &(state::warm_line(&dup).to_string() + "\n");
        warm += &(state::warm_line(&gone).to_string() + "\n");
        std::fs::write(home.join("routing/warm.jsonl"), warm).unwrap();
    })
    .await;
    let engine = f.setup.engine.clone();
    tokio::task::spawn_blocking(move || engine.journal.flush_blocking()).await.unwrap();

    let after = f.setup.engine.router.lock().warm.stored();
    assert_eq!(after.len(), live.len(), "the removed account's fingerprint is not loaded");
    let on_disk = lines(f.setup._dir.path(), "warm.jsonl");
    assert_eq!(on_disk.len(), after.len(), "one line per live fingerprint, no history");
    let ledgers = lines(f.setup._dir.path(), "ledger.jsonl");
    assert_eq!(ledgers.len(), 1, "one ledger line for the one target and tier in use");
    assert_eq!(ledgers[0]["target"], "u");
}
