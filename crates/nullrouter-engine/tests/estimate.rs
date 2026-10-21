//! Quota between polls, and without a quota report (spec 006, US7, T075, FR-021, FR-022, SC-008):
//! the estimate is the last poll less what 0router sent, a poll replaces it, a passed reset
//! counts as a reset, a failed poll keeps it running, and an account nobody reports on is paced
//! from its plugin's declared limits or is pay-as-you-go.

mod common;

use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::route;
use nullrouter_engine::routing::QuotaSource;
use nullrouter_engine::routing::view::{AccountView, TargetView, WindowView};
use nullrouter_engine::testkit::SimWindow;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const REPORTED: &str = r#"
[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
"#;

const COUNTED: &str = r#"
[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 100000
"#;

const WINDOW: f64 = 1_000_000.0;

fn at(offset_s: i64) -> String {
    let now = SystemTime::now();
    let t = if offset_s >= 0 {
        now + Duration::from_secs(offset_s as u64)
    } else {
        now - Duration::from_secs(offset_s.unsigned_abs())
    };
    nullrouter_engine::clock::rfc3339(t)
}

async fn fleet() -> FleetSetup {
    let f = Fleet::new()
        .provider("alpha", REPORTED, &[("5h", "tokens")])
        .account("alpha", "one", 1.0)
        .unified(&[("alpha", "m1")])
        .build()
        .await;
    f.quota.set("alpha/one", vec![SimWindow::new("5h", "tokens", WINDOW, &at(2 * 3600))]);
    f
}

fn view(f: &FleetSetup) -> TargetView {
    let st = f.setup.engine.snapshot();
    route::view_all(&f.setup.engine, &st, Some("u"), SystemTime::now()).remove(0)
}

fn window(f: &FleetSetup) -> WindowView {
    account(&view(f), "one").windows[0].clone()
}

fn account<'a>(v: &'a TargetView, name: &str) -> &'a AccountView {
    v.accounts.iter().find(|a| a.account == name).unwrap()
}

async fn serve(f: &FleetSetup, n: usize) {
    for i in 0..n {
        let body =
            json!({"model": "u", "stream": false, "messages": [{"role": "user", "content": format!("hello {i}")}]});
        let req = request(&f.setup, "openai-chat", "u", body, &format!("ak_{i}"), CancellationToken::new());
        let id = req.id.clone();
        f.setup.engine.text(f.setup.engine.snapshot(), req).await.expect("served");
        settled(&f.setup, &id).await;
    }
}

async fn poll(f: &FleetSetup, provider: &str, name: &str) -> bool {
    f.setup.engine.poll_quota(provider, name).await.is_some_and(|p| p.ok())
}

#[tokio::test]
async fn between_polls_the_estimate_drops_by_what_was_sent_and_a_poll_replaces_it() {
    let f = fleet().await;
    // Before the first poll the account is assumed on pace, with a full window.
    let first = view(&f);
    assert!(account(&first, "one").pending_first_poll);
    assert!(nullrouter_engine::routing::view::warnings(&first).iter().any(|l| l.contains("no poll yet")));
    assert_eq!(window(&f).remaining_now, WINDOW);

    assert!(poll(&f, "alpha", "one").await);
    assert!(!account(&view(&f), "one").pending_first_poll);
    let w = window(&f);
    assert_eq!((w.remaining_at_poll, w.cost_since_poll, w.remaining_now), (Some(WINDOW), 0.0, WINDOW));

    serve(&f, 3).await;
    let served = f.quota.used("alpha/one", "5h").unwrap();
    assert!(served > 0.0, "the provider counted what it served");
    let w = window(&f);
    assert_eq!(w.remaining_at_poll, Some(WINDOW), "the poll's figure stays until the next poll");
    assert_eq!(w.cost_since_poll, served, "the cost is what the provider charged, in the window's unit");
    assert_eq!(w.remaining_now, WINDOW - served);

    // SC-008: at the next poll, with no outside use, the estimate equals the provider's figure.
    let estimate = w.remaining_now;
    assert!(poll(&f, "alpha", "one").await);
    let w = window(&f);
    assert_eq!((w.remaining_at_poll, w.cost_since_poll), (Some(estimate), 0.0));
    assert_eq!(w.remaining_now, WINDOW - f.quota.used("alpha/one", "5h").unwrap());
}

#[tokio::test]
async fn a_poll_corrects_an_estimate_that_outside_use_made_wrong() {
    let f = fleet().await;
    assert!(poll(&f, "alpha", "one").await);
    serve(&f, 2).await;
    // The account was also used somewhere else.
    let used = f.quota.used("alpha/one", "5h").unwrap();
    f.quota.set("alpha/one", vec![SimWindow::new("5h", "tokens", WINDOW, &at(2 * 3600)).used(used + 250_000.0)]);
    assert!(account(&view(&f), "one").windows[0].remaining_now > WINDOW - used - 1.0);
    assert!(poll(&f, "alpha", "one").await);
    assert_eq!(account(&view(&f), "one").windows[0].remaining_now, WINDOW - used - 250_000.0);
}

#[tokio::test]
async fn a_reset_that_passed_before_the_next_poll_counts_as_a_reset() {
    let f = fleet().await;
    // The window reset an hour ago and the provider still reported it with 400k used at the poll.
    f.quota.set("alpha/one", vec![SimWindow::new("5h", "tokens", WINDOW, &at(-3600)).used(400_000.0)]);
    assert!(poll(&f, "alpha", "one").await);
    let v = view(&f);
    let w = &account(&v, "one").windows[0];
    assert_eq!((w.remaining_at_poll, w.remaining_now), (Some(600_000.0), WINDOW), "full again");
    let next = w.resets_at.expect("a reset");
    assert!(
        next > SystemTime::now() && next < SystemTime::now() + Duration::from_secs(5 * 3600),
        "one length on: {next:?}"
    );

    // What is sent after the reset counts against the new window.
    serve(&f, 2).await;
    let w = window(&f);
    assert!(w.remaining_now < WINDOW && w.cost_since_poll > 0.0, "{w:?}");
    assert_eq!(w.remaining_now, WINDOW - w.cost_since_poll);

    // The next poll confirms or corrects it.
    f.quota.reset("alpha/one", "5h", &at(5 * 3600));
    assert!(poll(&f, "alpha", "one").await);
    let w = window(&f);
    assert_eq!(w.remaining_now, WINDOW - f.quota.used("alpha/one", "5h").unwrap_or(0.0).min(WINDOW));
}

#[tokio::test]
async fn a_failing_poll_keeps_the_estimate_running_and_shows_stale() {
    let f = fleet().await;
    assert!(poll(&f, "alpha", "one").await);
    let polled = account(&view(&f), "one").polled_at.expect("polled");
    assert!(!account(&view(&f), "one").stale);

    // Traffic from before the failed poll still counts against the last good one.
    serve(&f, 1).await;
    let before = window(&f).cost_since_poll;
    assert!(before > 0.0);
    f.quota.fail(Some(500));
    assert!(!poll(&f, "alpha", "one").await);
    assert_eq!(window(&f).cost_since_poll, before, "a failed poll doesn't reset the count");
    serve(&f, 2).await;
    let v = view(&f);
    let a = account(&v, "one");
    assert!(a.stale, "the last poll failed");
    assert_eq!(a.polled_at, Some(polled), "the age is the last good poll's");
    let w = &a.windows[0];
    assert_eq!((w.remaining_at_poll, w.cost_since_poll > 0.0), (Some(WINDOW), true));
    assert_eq!(w.remaining_now, WINDOW - w.cost_since_poll, "the estimate kept running");
    assert!(nullrouter_engine::routing::view::warnings(&v).iter().any(|l| l.contains("stale")));
}

#[tokio::test]
async fn an_account_with_declared_limits_and_no_report_is_estimated_and_one_with_neither_is_pay_as_you_go() {
    let f = Fleet::new()
        .provider("alpha", COUNTED, &[])
        .provider("beta", "", &[])
        .account("alpha", "one", 1.0)
        .account("beta", "key", 1.0)
        .unified(&[("alpha", "m1"), ("beta", "m1")])
        .build()
        .await;
    let v = view(&f);
    let (one, key) = (account(&v, "one"), account(&v, "key"));
    assert_eq!((one.source, key.source), (QuotaSource::Estimated, QuotaSource::PayAsYouGo));
    assert_eq!(one.windows[0].remaining_now, 100_000.0);
    assert!(one.pace.is_some(), "an estimated account is paced");
    assert!(key.windows.is_empty());

    // Pin the traffic to the estimated account by giving the other none to take cold work.
    for i in 0..3 {
        let body =
            json!({"model": "alpha/m1", "stream": false, "messages": [{"role": "user", "content": format!("hi {i}")}]});
        let req = request(&f.setup, "openai-chat", "alpha/m1", body, &format!("ak_{i}"), CancellationToken::new());
        let id = req.id.clone();
        f.setup.engine.text(f.setup.engine.snapshot(), req).await.expect("served");
        settled(&f.setup, &id).await;
    }
    let w = window(&f);
    assert!(w.cost_since_poll > 0.0 && w.remaining_at_poll.is_none(), "{w:?}");
    assert_eq!(w.remaining_now, 100_000.0 - w.cost_since_poll);
    assert!(w.resets_at.is_some(), "an estimated window has a reset to pace by");
}

#[tokio::test]
async fn a_crash_loses_no_counted_traffic() {
    use nullrouter_engine::state::Engine;
    use nullrouter_registry::OperatorHome;

    let mut f = fleet().await;
    assert!(poll(&f, "alpha", "one").await);
    serve(&f, 2).await;
    // The tally is checkpointed; three more requests are sent after it and before the crash.
    f.setup.engine.checkpoint_tallies().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    serve(&f, 3).await;
    let before = f.setup.engine.history.tally.get("alpha", "one");
    let cost = window(&f).cost_since_poll;
    assert_eq!(before.values().map(|t| t.requests).sum::<u64>(), 5);

    // The process dies: no shutdown checkpoint. Only the journal's writer is stopped.
    let old = f.setup.engine.clone();
    tokio::task::spawn_blocking(move || old.journal.shutdown()).await.unwrap();
    let (engine, _) = Engine::open_parity(OperatorHome::new(f.setup._dir.path())).unwrap();
    let engine = std::sync::Arc::new(engine);
    let e = engine.clone();
    tokio::task::spawn_blocking(move || e.recover_journal()).await.unwrap().unwrap();

    assert_eq!(engine.history.tally.get("alpha", "one"), before, "the checkpoint plus the journal after it");
    f.setup.engine = engine;
    // Nothing is polled yet after a restart, so the window is full and on pace; what was sent counts
    // for the hourly counters an estimated window reads.
    let hours = f.setup.engine.history.tally.hours_since("alpha", "one", SystemTime::now() - Duration::from_secs(3600));
    let counted: u64 = hours.iter().flat_map(|(_, t)| t.values()).map(|t| t.requests).sum();
    assert_eq!(counted, 5);
    assert!(cost > 0.0);
}

/// Stops the journal's writer without a shutdown checkpoint, lets `edit` change the files the
/// crash left, and reopens the engine from them as `serve` does.
async fn crash_and_recover(f: &mut FleetSetup, edit: impl FnOnce(&std::path::Path)) {
    use nullrouter_engine::state::Engine;
    use nullrouter_registry::OperatorHome;

    let old = f.setup.engine.clone();
    tokio::task::spawn_blocking(move || old.journal.shutdown()).await.unwrap();
    edit(f.setup._dir.path());
    let (engine, _) = Engine::open_parity(OperatorHome::new(f.setup._dir.path())).unwrap();
    let engine = std::sync::Arc::new(engine);
    let e = engine.clone();
    tokio::task::spawn_blocking(move || e.recover_journal()).await.unwrap().unwrap();
    f.setup.engine = engine;
}

/// Each journal segment under `home/records`, with its lines parsed.
fn journal_lines(home: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<serde_json::Value>)> {
    let mut days = Vec::new();
    for e in std::fs::read_dir(home.join("records")).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "jsonl") {
            let text = std::fs::read_to_string(e.path()).unwrap();
            days.push((e.path(), text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()));
        }
    }
    days
}

fn edit_json(path: &std::path::Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    edit(&mut v);
    std::fs::write(path, v.to_string()).unwrap();
}

#[tokio::test]
async fn an_attempt_the_checkpoint_holds_is_not_recounted_when_its_journal_time_is_later() {
    let mut f = fleet().await;
    assert!(poll(&f, "alpha", "one").await);
    serve(&f, 2).await;
    f.setup.engine.checkpoint_tallies().await;
    let before = f.setup.engine.history.tally.get("alpha", "one");
    assert_eq!(before.values().map(|t| t.requests).sum::<u64>(), 2);

    // The checkpoint holds both attempts, but its time is no later than the second one's time in
    // the journal: both fell in the same millisecond (the checkpoint's time is cut to it), or the
    // wall clock stepped back in between. The order the tally saw is what counts.
    crash_and_recover(&mut f, |home| {
        let last = journal_lines(home)
            .into_iter()
            .flat_map(|(_, lines)| lines)
            .filter_map(|r| r["arrived"].as_str().map(str::to_owned))
            .max()
            .unwrap();
        edit_json(&home.join("quota/alpha/one.tally.json"), |cp| cp["at"] = json!(last));
    })
    .await;
    assert_eq!(f.setup.engine.history.tally.get("alpha", "one"), before, "each attempt counted once");
}

#[tokio::test]
async fn a_checkpoint_written_before_tally_sequences_still_recovers_by_time() {
    let mut f = fleet().await;
    assert!(poll(&f, "alpha", "one").await);
    serve(&f, 2).await;
    f.setup.engine.checkpoint_tallies().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    serve(&f, 3).await;
    let before = f.setup.engine.history.tally.get("alpha", "one");
    assert_eq!(before.values().map(|t| t.requests).sum::<u64>(), 5);

    // An upgrade: the old binary wrote the checkpoint and the journal lines before it, with no
    // sequence; the new one wrote the lines after it. Those are told apart by time, as before.
    crash_and_recover(&mut f, |home| {
        let cp = home.join("quota/alpha/one.tally.json");
        let mut at = String::new();
        edit_json(&cp, |v| {
            v.as_object_mut().unwrap().remove("seq");
            at = v["at"].as_str().unwrap().to_owned();
        });
        let at = nullrouter_engine::clock::parse_rfc3339(&at).unwrap();
        let mut stripped = std::collections::BTreeSet::new();
        for (path, mut lines) in journal_lines(home) {
            let old: Vec<serde_json::Value> = lines
                .iter()
                .filter(|l| {
                    l["arrived"].as_str().and_then(nullrouter_engine::clock::parse_rfc3339).is_some_and(|t| t < at)
                })
                .map(|l| l["id"].clone())
                .collect();
            for l in lines.iter_mut().filter(|l| l["t"] == "attempt" && old.contains(&l["id"])) {
                if l["attempt"].as_object_mut().unwrap().remove("tally_seq").is_some() {
                    stripped.insert((l["id"].to_string(), l["attempt"]["n"].to_string()));
                }
            }
            let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
            std::fs::write(path, text).unwrap();
        }
        assert_eq!(stripped.len(), 2, "the two attempts before the checkpoint carried a sequence");
    })
    .await;
    assert_eq!(f.setup.engine.history.tally.get("alpha", "one"), before, "the checkpoint plus the journal after it");
}
