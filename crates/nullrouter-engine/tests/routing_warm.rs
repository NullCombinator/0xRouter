//! Warm requests stay where their cache is (spec 006, US1, T020): an agent's follow-ups stay on
//! the account that holds its prefix, and move only when that account can't serve.

mod common;

use std::time::Duration;

use common::*;
use nullrouter_engine::classify;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::routing::{MovedBecause, PlacementReason};
use nullrouter_engine::testkit::SimWindow;
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

const PAYG: &str = "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n";

const SYSTEM: &str = "You are a careful assistant. Answer in one short sentence and never guess.";

fn body(turns: &[&str]) -> Value {
    let mut messages = vec![json!({"role": "system", "content": SYSTEM})];
    for (i, t) in turns.iter().enumerate() {
        messages.push(json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": t}));
    }
    json!({"model": "u", "stream": false, "messages": messages})
}

/// One request of `agent` over `turns`, settled.
async fn turn(f: &FleetSetup, agent: &str, turns: &[&str]) -> RequestRecord {
    let req = request(&f.setup, "openai-chat", "u", body(turns), agent, CancellationToken::new());
    let id = req.id.clone();
    if let Err(e) = f.setup.engine.text(f.setup.engine.snapshot(), req).await {
        panic!("not served: {e:?}");
    }
    settled(&f.setup, &id).await
}

fn served(r: &RequestRecord) -> String {
    let s = r.served_by.as_ref().expect("served");
    format!("{}/{}", s.provider, s.account.as_deref().unwrap_or(""))
}

fn reason(r: &RequestRecord) -> PlacementReason {
    r.attempts.last().and_then(|a| a.placement).expect("a placement").reason
}

fn moved(r: &RequestRecord) -> Option<MovedBecause> {
    r.decision.as_ref().and_then(|d| d.warm.as_ref()).and_then(|w| w.moved_because)
}

fn two_accounts() -> Fleet {
    Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .account("alpha", "one", 1.0)
        .account("alpha", "two", 1.0)
        .unified(&[("alpha", "m1")])
}

async fn rest(f: &FleetSetup, account: &str, status: u16) {
    f.setup.engine.cooldowns.fail("alpha", account, "m1", &classify::upstream(status, "x"));
}

#[tokio::test]
async fn follow_ups_stay_on_the_account_that_holds_the_cache() {
    let f = two_accounts().build().await;
    let first = turn(&f, "ak_a", &["one"]).await;
    assert_eq!(served(&first), "alpha/one");
    assert_eq!(reason(&first), PlacementReason::ColdByDeficit);
    assert!(first.decision.as_ref().unwrap().warm.is_none());
    let reads = f.cache.cache_reads("alpha/one");

    let second = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!(served(&second), "alpha/one");
    assert_eq!(reason(&second), PlacementReason::Warm);
    let hit = second.decision.as_ref().unwrap().warm.clone().unwrap();
    assert!(hit.stayed && hit.moved_because.is_none());
    assert!(hit.prefix_tokens > 0);
    assert!(f.cache.cache_reads("alpha/one") > reads, "the provider reports cache reads on the warm request");
}

#[tokio::test]
async fn a_warm_account_stays_though_another_comes_first_and_a_new_session_is_warm_too() {
    let f = two_accounts().build().await;
    // `one` rests, so the agent lands on `two`; once it is back, `one` is first in order again.
    rest(&f, "one", 429).await;
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/two");
    tokio::time::sleep(Duration::from_millis(2100)).await;

    let follow = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!((served(&follow).as_str(), reason(&follow)), ("alpha/two", PlacementReason::Warm));

    // A new conversation with the same system prompt: only that prefix is shared, and it is warm.
    let fresh = turn(&f, "ak_a", &["something else entirely"]).await;
    assert_eq!((served(&fresh).as_str(), reason(&fresh)), ("alpha/two", PlacementReason::Warm));

    // Another agent with identical prompts shares nothing: it is cold, and goes by order.
    let other = turn(&f, "ak_b", &["one", "two", "three"]).await;
    assert!(other.decision.as_ref().unwrap().warm.is_none());
    assert_eq!((served(&other).as_str(), reason(&other)), ("alpha/one", PlacementReason::ColdByDeficit));
}

#[tokio::test]
async fn a_rate_limited_warm_account_moves_for_capacity() {
    let f = two_accounts().build().await;
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/one");
    rest(&f, "one", 429).await;

    let r = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!(served(&r), "alpha/two");
    assert_eq!(reason(&r), PlacementReason::MovedForCapacity);
    assert_eq!(moved(&r), Some(MovedBecause::RateLimited));
    assert!(!r.decision.unwrap().warm.unwrap().stayed);
}

#[tokio::test]
async fn a_window_at_its_floor_moves_warm_work() {
    let f = two_accounts().build().await;
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/one");
    // 99% of the window used: below the 5% reserve.
    f.quota.set("alpha/one", vec![SimWindow::new("5h", "tokens", 1_000_000.0, "2099-01-01T00:00:00Z").used(990_000.0)]);
    f.setup.engine.poll_quota("alpha", "one").await.expect("polled");

    let r = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!(served(&r), "alpha/two");
    assert_eq!(reason(&r), PlacementReason::MovedForCapacity);
    assert_eq!(moved(&r), Some(MovedBecause::ReserveFloor));
}

#[tokio::test]
async fn warm_on_pay_as_you_go_leaves_once_a_subscription_can_serve() {
    let f = Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .provider("pay", PAYG, &[])
        .account("alpha", "one", 1.0)
        .account("pay", "key", 1.0)
        .unified(&[("alpha", "m1"), ("pay", "m1")])
        .build()
        .await;
    rest(&f, "one", 429).await;
    let first = turn(&f, "ak_a", &["one"]).await;
    assert_eq!((served(&first).as_str(), reason(&first)), ("pay/key", PlacementReason::Overflow));

    // The subscription is still resting: nothing can take the request off pay-as-you-go.
    let second = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!((served(&second).as_str(), reason(&second)), ("pay/key", PlacementReason::Warm));

    tokio::time::sleep(Duration::from_millis(2100)).await;
    let third = turn(&f, "ak_a", &["one", "two", "three", "four", "five"]).await;
    assert_eq!((served(&third).as_str(), reason(&third)), ("alpha/one", PlacementReason::LeftPayAsYouGo));
    assert_eq!(moved(&third), Some(MovedBecause::LeftPayAsYouGo));
}

#[tokio::test]
async fn an_idle_prefix_past_the_cache_lifetime_is_cold() {
    let f = Fleet::new()
        .provider("alpha", SUB_SHORT_CACHE, &[("5h", "tokens")])
        .account("alpha", "one", 1.0)
        .account("alpha", "two", 1.0)
        .unified(&[("alpha", "m1")])
        .build()
        .await;
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/one");
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let r = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert!(r.decision.as_ref().unwrap().warm.is_none(), "past its lifetime nothing is warm");
    assert_eq!(reason(&r), PlacementReason::ColdByDeficit);
}

#[tokio::test]
async fn the_longest_prefix_beats_the_system_prompt_alone() {
    let f = two_accounts().build().await;
    // `one` holds only the system prompt and the first turn; `two` holds a longer conversation.
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/one");
    rest(&f, "one", 429).await;
    assert_eq!(served(&turn(&f, "ak_a", &["one", "two", "three"]).await), "alpha/two");
    tokio::time::sleep(Duration::from_millis(2100)).await;

    let r = turn(&f, "ak_a", &["one", "two", "three", "four", "five"]).await;
    assert_eq!((served(&r).as_str(), reason(&r)), ("alpha/two", PlacementReason::Warm));
    assert!(r.decision.unwrap().warm.unwrap().prefix_tokens > 0);
}

fn rewrite_accounts(f: &FleetSetup, edit: impl FnOnce(String) -> String) {
    let path = f.setup._dir.path().join(nullrouter_engine::accounts::FILE);
    let text = edit(std::fs::read_to_string(&path).unwrap());
    nullrouter_engine::files::write_private(&path, &text).unwrap();
}

#[tokio::test]
async fn a_disabled_warm_account_is_unusable_and_the_request_goes_elsewhere() {
    let f = two_accounts().build().await;
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/one");
    rewrite_accounts(&f, |t| t.replacen("name = \"one\"\n", "name = \"one\"\ndisabled = true\n", 1));
    f.setup.engine.reload().await.unwrap();

    let r = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!(served(&r), "alpha/two");
    assert_eq!(moved(&r), Some(MovedBecause::WarmUnusable));
}

#[tokio::test]
async fn priority_zero_keeps_its_warm_work_but_takes_no_cold_work() {
    let f = two_accounts().build().await;
    assert_eq!(served(&turn(&f, "ak_a", &["one"]).await), "alpha/one");
    rewrite_accounts(&f, |t| t.replacen("name = \"one\"\n", "name = \"one\"\npriority = 0.0\n", 1));
    f.setup.engine.reload().await.unwrap();

    let warm = turn(&f, "ak_a", &["one", "two", "three"]).await;
    assert_eq!((served(&warm).as_str(), reason(&warm)), ("alpha/one", PlacementReason::Warm));
    let cold = turn(&f, "ak_b", &["unrelated"]).await;
    assert_eq!(served(&cold), "alpha/two", "priority 0 is never cold work");
}
