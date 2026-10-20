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

// ---- Spec 012, story 3: the view's meter block (T034) ----

mod meter_block {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use nullrouter_engine::quota::extract::rfc3339_millis;
    use nullrouter_engine::quota::fit::learner::{SetAsideCounts, WindowSnapshot};
    use nullrouter_engine::quota::fit::outside::{self, Line, OutsideEntry, OutsideType};
    use nullrouter_engine::quota::fit::{
        AccountMeters, MeterNumber, NumberOverrides, NumberState, Progress, Source, WindowOverrides,
    };
    use nullrouter_engine::routing::view::window_meter_view;
    use nullrouter_registry::schema::MeterDecl;
    use serde_json::json;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    fn declared() -> MeterDecl {
        toml::from_str(
            "name = \"5-hour\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 9000000\n\
             token_weights = { input = 2.0, output = 10.0, cache_read = 0.2, cache_write = 2.5 }\n",
        )
        .unwrap()
    }

    #[test]
    fn every_number_carries_what_the_operator_reads() {
        let meter = declared();
        let fitted_since = at(100);
        let mut numbers = BTreeMap::new();
        numbers.insert("capacity@one".to_owned(), NumberState::Fitted { since: fitted_since });
        numbers.insert("weight.input".to_owned(), NumberState::Yardstick);
        numbers.insert(
            "weight.output".to_owned(),
            NumberState::Learning { progress: Progress { intervals: 40, half_width: 0.3 } },
        );
        numbers.insert("weight.cache_read".to_owned(), NumberState::NotSeparable { partner: "weight.cache_write".to_owned() });
        let mut ranges = BTreeMap::new();
        ranges.insert("capacity@one".to_owned(), (13_800_000.0, 13_100_000.0, 14_600_000.0));
        // A weight on a percent window is a ratio to the input weight (2.0 in effect).
        ranges.insert("weight.output".to_owned(), (4.0, 3.0, 5.0));
        let snap = WindowSnapshot {
            epoch: at(0),
            percent: true,
            numbers,
            ranges,
            splits: BTreeMap::new(),
            set_aside: [("one".to_owned(), SetAsideCounts { reset: 3, usage_unreported: 12, exhausted: 0 })].into(),
        };
        let ov = WindowOverrides {
            plugin: NumberOverrides { weights: [None, Some(6.0), None, None], ..NumberOverrides::default() },
            accounts: [("one".to_owned(), NumberOverrides { capacity: Some(12_000_000.0), ..NumberOverrides::default() })].into(),
        };
        let mut in_effect = meter.clone();
        in_effect.capacity = Some(12_000_000.0);
        let cell = AccountMeters {
            windows: vec![in_effect].into(),
            sources: [(
                "5-hour".to_owned(),
                [(MeterNumber::Capacity, Source::AccountOverride)].into(),
            )]
            .into(),
        };

        let view = window_meter_view(&meter, Some(&snap), Some(&ov), "one", Some(&cell));
        let v = serde_json::to_value(&view).unwrap();
        assert_eq!(v["window"], "5-hour");
        assert_eq!(v["epoch"], json!(rfc3339_millis(at(0))));
        assert_eq!(v["split"], json!(null));
        assert_eq!(v["set_aside"], json!({"reset": 3, "usage_unreported": 12, "exhausted": 0}));
        let n = v["numbers"].as_array().unwrap();
        let names: Vec<&str> = n.iter().map(|x| x["number"].as_str().unwrap()).collect();
        assert_eq!(names, ["capacity", "weight.input", "weight.output", "weight.cache_read"]);

        // Fitted: the override wins, the fit stays visible.
        assert_eq!(n[0]["declared"], json!(9_000_000.0));
        assert_eq!(n[0]["account_override"], json!(12_000_000.0));
        assert_eq!(n[0]["plugin_override"], json!(null));
        assert_eq!(n[0]["fit"], json!({"value": 13_800_000.0, "low": 13_100_000.0, "high": 14_600_000.0}));
        assert_eq!(n[0]["in_use"], json!(12_000_000.0));
        assert_eq!(n[0]["source"], "account_override");
        assert_eq!(n[0]["state"], "fitted");
        assert_eq!(n[0]["since"], json!(rfc3339_millis(fitted_since)));
        assert_eq!(n[0]["progress"], json!(null));

        // Yardstick: held, never fitted.
        assert_eq!(n[1]["state"], "yardstick");
        assert_eq!(n[1]["fit"], json!(null));
        assert_eq!(n[1]["in_use"], json!(2.0));
        assert_eq!(n[1]["source"], "declared");

        // Learning: still has its range, scaled to the input weight; the plugin override shows.
        assert_eq!(n[2]["state"], "learning");
        assert_eq!(n[2]["declared"], json!(10.0));
        assert_eq!(n[2]["plugin_override"], json!(6.0));
        assert_eq!(n[2]["fit"], json!({"value": 8.0, "low": 6.0, "high": 10.0}));
        assert_eq!(n[2]["progress"], json!({"intervals": 40, "half_width": 0.3}));
        assert_eq!(n[2]["since"], json!(null));

        // Not separable: stays declared and names its partner.
        assert_eq!(n[3]["state"], "not_separable");
        assert_eq!(n[3]["partner"], "weight.cache_write");
        assert_eq!(n[3]["fit"], json!(null));
        assert_eq!(n[3]["in_use"], json!(0.2));
    }

    #[test]
    fn a_split_account_shows_its_split_and_a_window_nothing_learned_has_no_numbers() {
        let meter = declared();
        let snap = WindowSnapshot {
            epoch: at(0),
            percent: false,
            numbers: [("weight.output".to_owned(), NumberState::Fitted { since: at(5) })].into(),
            ranges: BTreeMap::new(),
            splits: [("one".to_owned(), (at(9), "weight.output 2.1x the pooled value".to_owned()))].into(),
            set_aside: BTreeMap::new(),
        };
        let v = serde_json::to_value(window_meter_view(&meter, Some(&snap), None, "one", None)).unwrap();
        assert_eq!(v["split"]["reason"], "weight.output 2.1x the pooled value");
        assert_eq!(v["split"]["since"], json!(rfc3339_millis(at(9))));
        assert_eq!(v["numbers"][0]["source"], "declared");
        assert_eq!(v["set_aside"], json!({"reset": 0, "usage_unreported": 0, "exhausted": 0}));

        let none = serde_json::to_value(window_meter_view(&meter, None, None, "one", None)).unwrap();
        assert_eq!(none["numbers"], json!([]));
        assert_eq!(none["epoch"], json!(null));
    }

    fn outside_entry(id: &str, ty: OutsideType, start: SystemTime) -> OutsideEntry {
        OutsideEntry {
            v: outside::VERSION,
            id: id.to_owned(),
            window: "5h".to_owned(),
            ty,
            start: rfc3339_millis(start),
            end: None,
            amount: (ty != OutsideType::Steady).then_some(4.0),
            rate_per_hour: (ty == OutsideType::Steady).then_some(0.2),
            part: (ty == OutsideType::Steady).then(|| "08-12".to_owned()),
            unit: "percent".to_owned(),
            found_at: rfc3339_millis(start),
        }
    }

    #[tokio::test]
    async fn the_view_carries_outside_use_alerts_and_the_fit_note() {
        let f = Fleet::new()
            .provider("alpha", SUB, &[("5h", "tokens")])
            .provider("beta", "", &[])
            .provider("gamma", SUB, &[])
            .account("alpha", "one", 1.0)
            .account("beta", "pay", 1.0)
            .account("gamma", "quiet", 1.0)
            .unified(&[("alpha", "m1"), ("beta", "m1"), ("gamma", "m1")])
            .build()
            .await;
        let now = SystemTime::now();
        let hour = Duration::from_secs(3600);
        let lines = [
            Line::Entry(outside_entry("01A", OutsideType::Idle, now - 30 * 24 * hour)),
            Line::Entry(outside_entry("01B", OutsideType::Busy, now - 2 * hour)),
            Line::Entry(outside_entry("01C", OutsideType::Steady, now - hour)),
            Line::Alert { v: outside::VERSION, id: "AL1".to_owned(), entry: "01B".to_owned(), raised_at: rfc3339_millis(now), text: None },
            Line::Alert { v: outside::VERSION, id: "AL2".to_owned(), entry: "01B".to_owned(), raised_at: rfc3339_millis(now), text: None },
            Line::Ack { v: outside::VERSION, alert: "AL1".to_owned(), at: rfc3339_millis(now) },
        ];
        outside::append(f.setup._dir.path(), "alpha", "one", &lines).unwrap();

        let v = view(&f);
        let by = |name: &str| v.accounts.iter().find(|a| a.account == name).unwrap();

        let one = by("one");
        assert_eq!(one.fit_note, None);
        assert_eq!(one.meter.len(), 1);
        assert_eq!(one.meter[0].window, "5h");
        assert_eq!(one.outside_use.intervals_7d, 1, "the month-old idle entry is out of the week");
        assert_eq!(one.outside_use.last.as_ref().unwrap().id, "01C");
        assert_eq!(one.outside_use.steady.len(), 1);
        assert_eq!(one.outside_use.steady[0].part, "08-12");
        let alerts: Vec<&str> = one.outside_use.alerts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(alerts, ["AL2"], "AL1 was acknowledged");

        assert_eq!(by("pay").fit_note.as_deref(), Some("pay-as-you-go"));
        assert!(by("pay").meter.is_empty());
        assert_eq!(by("quiet").fit_note.as_deref(), Some("provider reports no quota"));
    }
}
