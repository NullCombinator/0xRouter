//! Pay-as-you-go is overflow only (spec 006, US3, T044): it serves while no subscription can,
//! split by priority ÷ the price in effect now, with an operator's flat price replacing the
//! plugin's schedule, priority 0 receiving nothing and a floor-held subscription serving as the
//! last resort. The crossing into an off-peak period is covered at the placement level
//! (`routing::place`), where the clock is an argument.

mod common;

use common::*;
use nullrouter_engine::classify;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::routing::{DecisionKind, PlacementReason, WhyNot};
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

fn payg(input: f64) -> String {
    format!(
        "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.price]]\ninput = {input:?}\noutput = {:?}\n",
        input * 4.0
    )
}

fn body(text: &str) -> Value {
    json!({"model": "u", "stream": false, "messages": [
        {"role": "system", "content": "You are a careful assistant."},
        {"role": "user", "content": text},
    ]})
}

async fn cold(f: &FleetSetup, agent: &str, text: &str) -> RequestRecord {
    let req = request(&f.setup, "openai-chat", "u", body(text), agent, CancellationToken::new());
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

/// Two subscriptions on `alpha`, and pay-as-you-go keys at $1 and $4 per million.
fn fleet() -> Fleet {
    Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .provider("cheap", &payg(1.0), &[])
        .provider("dear", &payg(4.0), &[])
        .account("alpha", "a", 1.0)
        .account("alpha", "b", 1.0)
        .account("cheap", "key", 1.0)
        .account("dear", "key", 1.0)
        .unified(&[("alpha", "m1"), ("cheap", "m1"), ("dear", "m1")])
}

/// Both subscriptions below their reserve floor: 99% of the window used, reset far away.
async fn floor_both(f: &FleetSetup) {
    for a in ["a", "b"] {
        f.quota.set(
            &format!("alpha/{a}"),
            vec![SimWindow::new("5h", "tokens", 1_000_000.0, "2099-01-01T00:00:00Z").used(990_000.0)],
        );
        f.setup.engine.poll_quota("alpha", a).await.expect("polled");
    }
}

#[tokio::test]
async fn nothing_goes_to_pay_as_you_go_while_a_subscription_can_serve() {
    let f = fleet().build().await;
    for i in 0..8 {
        let r = cold(&f, &format!("ak_{i}"), &format!("question {i}")).await;
        assert!(served(&r).starts_with("alpha/"), "{}", served(&r));
        let d = r.decision.unwrap();
        assert_eq!(d.kind, DecisionKind::Cold);
        // The keys were candidates, and could have served; they were behind the subscriptions.
        let key = d.candidates.iter().find(|c| c.provider == "cheap").unwrap();
        assert!(key.eligible && key.price_now == Some(1.0));
    }
    assert_eq!(f.cache.requests("cheap/key") + f.cache.requests("dear/key"), 0);
}

#[tokio::test]
async fn overflow_is_split_by_priority_over_price_and_the_record_says_why_each_subscription_was_passed() {
    let f = fleet().build().await;
    floor_both(&f).await;
    let (mut cheap, mut dear) = (0, 0);
    let mut first = None;
    for i in 0..10 {
        let r = cold(&f, &format!("ak_{i}"), &format!("question {i}")).await;
        first.get_or_insert_with(|| r.clone());
        match served(&r).as_str() {
            "cheap/key" => cheap += 1,
            "dear/key" => dear += 1,
            other => panic!("served by {other}"),
        }
    }
    // Shares are 80% and 20% (1 ÷ $1 against 1 ÷ $4).
    assert!(cheap >= 6 && dear >= 1, "cheap {cheap}, dear {dear}");
    let r = first.unwrap();
    assert_eq!(reason(&r), PlacementReason::Overflow);
    let d = r.decision.unwrap();
    assert_eq!(d.kind, DecisionKind::Overflow);
    for a in ["a", "b"] {
        let row = d.candidates.iter().find(|c| c.provider == "alpha" && c.account == a).unwrap();
        assert!(!row.eligible);
        assert_eq!(row.why_not, Some(WhyNot::ReserveFloor));
    }
    let cheap_row = d.candidates.iter().find(|c| c.provider == "cheap").unwrap();
    let dear_row = d.candidates.iter().find(|c| c.provider == "dear").unwrap();
    assert_eq!((cheap_row.price_now, dear_row.price_now), (Some(1.0), Some(4.0)));
    assert!(cheap_row.share.unwrap() > dear_row.share.unwrap());
}

#[tokio::test]
async fn the_operators_price_replaces_the_plugins_schedule() {
    // The plugin says $4, the operator says $0.25: that key takes the overflow.
    let f = fleet().price("dear", "key", 0.25).build().await;
    floor_both(&f).await;
    let r = cold(&f, "ak_1", "hello").await;
    assert_eq!(served(&r), "dear/key");
    let row = r.decision.unwrap().candidates.into_iter().find(|c| c.provider == "dear").unwrap();
    assert_eq!(row.price_now, Some(0.25));
}

#[tokio::test]
async fn a_priority_zero_key_receives_nothing() {
    let f = Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .provider("cheap", &payg(1.0), &[])
        .provider("dear", &payg(4.0), &[])
        .account("alpha", "a", 1.0)
        .account("cheap", "key", 0.0)
        .account("dear", "key", 1.0)
        .unified(&[("alpha", "m1"), ("cheap", "m1"), ("dear", "m1")])
        .build()
        .await;
    f.quota.set("alpha/a", vec![SimWindow::new("5h", "tokens", 1_000_000.0, "2099-01-01T00:00:00Z").used(990_000.0)]);
    f.setup.engine.poll_quota("alpha", "a").await.expect("polled");
    for i in 0..4 {
        let r = cold(&f, &format!("ak_{i}"), &format!("q {i}")).await;
        assert_eq!(served(&r), "dear/key");
        let row = r.decision.unwrap().candidates.into_iter().find(|c| c.provider == "cheap").unwrap();
        assert_eq!(row.why_not, Some(WhyNot::PriorityZero));
    }
}

#[tokio::test]
async fn with_everything_else_unavailable_a_floor_held_subscription_serves_as_the_last_resort() {
    let f = fleet().build().await;
    floor_both(&f).await;
    for p in ["cheap", "dear"] {
        f.setup.engine.cooldowns.fail(p, "key", "m1", &classify::upstream(429, "slow down"));
    }
    let r = cold(&f, "ak_1", "hello").await;
    assert!(served(&r).starts_with("alpha/"), "{}", served(&r));
    assert_eq!(reason(&r), PlacementReason::LastResort);
}

#[tokio::test]
async fn when_only_priority_zero_accounts_could_serve_the_client_gets_the_error_with_every_reason() {
    let f = Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .provider("cheap", &payg(1.0), &[])
        .account("alpha", "a", 0.0)
        .account("cheap", "key", 0.0)
        .unified(&[("alpha", "m1"), ("cheap", "m1")])
        .build()
        .await;
    let req = request(&f.setup, "openai-chat", "u", body("hello"), "ak_1", CancellationToken::new());
    let id = req.id.clone();
    let failure = f.setup.engine.text(f.setup.engine.snapshot(), req).await.err().expect("nothing may serve");
    assert_eq!(failure.status, 503);
    assert!(failure.message.contains("alpha/a"), "{}", failure.message);
    assert!(failure.message.contains("cheap/key"), "{}", failure.message);
    assert_eq!(failure.message.matches("priority 0").count(), 2, "{}", failure.message);
    // The record keeps the same table.
    let d = f.setup.engine.records.get(&id).unwrap().decision.unwrap();
    assert!(d.candidates.iter().all(|c| c.why_not == Some(WhyNot::PriorityZero)));
}
