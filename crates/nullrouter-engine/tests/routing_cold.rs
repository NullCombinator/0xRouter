//! Cold work spends subscription quota before it expires (spec 006, US2, T033): shares follow
//! pace-weighted rates, a request-counted window is compared by the size of the request, priority 0
//! takes no cold work, concurrent placements don't pile onto one account, and the record keeps
//! every candidate's row.

mod common;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::routing::PlacementReason;
use nullrouter_engine::testkit::SimWindow;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const TOKENS: &str = r#"
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

const REQUESTS: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "5m"
min_tokens = 0

[[routing.window]]
name = "5h"
length = "5h"
unit = "requests"
capacity = 500
"#;

fn body(text: &str) -> Value {
    json!({"model": "u", "stream": false, "messages": [
        {"role": "system", "content": "You are a careful assistant."},
        {"role": "user", "content": text},
    ]})
}

/// One cold request: every agent is new, so nothing is warm.
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

fn reset_in(d: Duration) -> String {
    nullrouter_engine::clock::rfc3339(SystemTime::now() + d)
}

const H: Duration = Duration::from_secs(3600);

fn two_tokens() -> Fleet {
    Fleet::new()
        .provider("alpha", TOKENS, &[("5h", "tokens")])
        .account("alpha", "a", 1.0)
        .account("alpha", "b", 1.0)
        .unified(&[("alpha", "m1")])
}

#[tokio::test]
async fn the_account_whose_quota_expires_unspent_takes_the_larger_share() {
    let f = two_tokens().build().await;
    // A has 80% left with 20% of its window to go; B has 50% left with 50% to go.
    f.quota.set("alpha/a", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H)).used(200_000.0)]);
    f.quota.set("alpha/b", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H * 5 / 2)).used(500_000.0)]);
    f.setup.engine.poll_quota("alpha", "a").await.expect("polled");
    f.setup.engine.poll_quota("alpha", "b").await.expect("polled");

    let (mut a, mut b) = (0, 0);
    let mut first = None;
    for i in 0..30 {
        let r = cold(&f, &format!("ak_{i}"), &format!("question {i}")).await;
        first.get_or_insert_with(|| r.clone());
        match served(&r).as_str() {
            "alpha/a" => a += 1,
            _ => b += 1,
        }
    }
    // Shares are about 94% and 6%: A far ahead, and B still not starved.
    assert!(a >= 24 && b >= 1, "a {a}, b {b}");
    let d = first.unwrap().decision.unwrap();
    let row = |name: &str| d.candidates.iter().find(|r| r.account == name).unwrap().clone();
    assert!(row("a").pace.unwrap() > row("b").pace.unwrap());
    assert!(row("a").share.unwrap() > row("b").share.unwrap());
}

#[tokio::test]
async fn the_decision_keeps_every_candidate_and_the_order_walked() {
    let f = two_tokens().config("[routing]\namortization = \"2h\"\n").build().await;
    let r = cold(&f, "ak_1", "hello").await;
    let d = r.decision.unwrap();
    assert_eq!(d.candidates.len(), 2);
    assert_eq!(d.order.len(), 2);
    assert_eq!(d.amortization_window.length, Duration::from_secs(7200));
    assert!(d.candidates.iter().all(|c| c.eligible && c.deficit_before.is_some() && c.weight.is_some()));
    assert_eq!(r.attempts.last().unwrap().placement.unwrap().reason, PlacementReason::ColdByDeficit);
}

#[tokio::test]
async fn a_request_counted_window_is_compared_by_the_size_of_the_request() {
    let f = Fleet::new()
        .provider("req", REQUESTS, &[("5h", "requests")])
        .provider("tok", TOKENS, &[("5h", "tokens")])
        .account("req", "a", 1.0)
        .account("tok", "b", 1.0)
        .unified(&[("req", "m1"), ("tok", "m1")])
        .build()
        .await;
    // Both on pace (20% left, 20% of the time left). A has 100 requests left, B 200,000 tokens
    // (about 55 tokens a second against A's 100 requests an hour).
    f.quota.set("req/a", vec![SimWindow::new("5h", "requests", 500.0, &reset_in(H)).used(400.0)]);
    f.quota.set("tok/b", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H)).used(800_000.0)]);
    f.setup.engine.poll_quota("req", "a").await.expect("polled");
    f.setup.engine.poll_quota("tok", "b").await.expect("polled");

    // A large request favours the request-counted account, a small one the token-counted account.
    let large = cold(&f, "ak_large", &"word ".repeat(40_000)).await;
    assert_eq!(served(&large), "req/a");
    let small = cold(&f, "ak_small", "hi").await;
    assert_eq!(served(&small), "tok/b");
}

#[tokio::test]
async fn priority_zero_takes_no_cold_work() {
    let f = Fleet::new()
        .provider("alpha", TOKENS, &[("5h", "tokens")])
        .account("alpha", "a", 0.0)
        .account("alpha", "b", 1.0)
        .unified(&[("alpha", "m1")])
        .build()
        .await;
    for i in 0..6 {
        let r = cold(&f, &format!("ak_{i}"), &format!("q {i}")).await;
        assert_eq!(served(&r), "alpha/b");
        let row = r.decision.unwrap().candidates.into_iter().find(|c| c.account == "a").unwrap();
        assert!(!row.eligible);
        assert_eq!(row.why_not, Some(nullrouter_engine::routing::WhyNot::PriorityZero));
    }
}

#[tokio::test]
async fn concurrent_cold_requests_do_not_all_land_on_the_largest_deficit() {
    let f = Arc::new(two_tokens().build().await);
    let sent: Vec<_> = (0..10)
        .map(|i| {
            let f = f.clone();
            tokio::spawn(async move { served(&cold(&f, &format!("ak_{i}"), &format!("question {i}")).await) })
        })
        .collect();
    let (mut a, mut b) = (0, 0);
    for s in sent {
        if s.await.unwrap() == "alpha/a" {
            a += 1;
        } else {
            b += 1;
        }
    }
    assert!(a >= 3 && b >= 3, "split {a}/{b}");
}

// ---- parity deviations D-006-1 to D-006-3 (T041), tests/parity/deviations.toml ----

#[tokio::test]
async fn d_006_1_order_comes_from_pace_not_fill_first() {
    // `first` leads the operator's order, but `second` has the quota that expires unspent.
    let f = Fleet::new()
        .provider("alpha", TOKENS, &[("5h", "tokens")])
        .account("alpha", "first", 1.0)
        .account("alpha", "second", 1.0)
        .unified(&[("alpha", "m1")])
        .build()
        .await;
    f.quota.set("alpha/first", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H * 5 / 2)).used(500_000.0)]);
    f.quota.set("alpha/second", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H)).used(200_000.0)]);
    f.setup.engine.poll_quota("alpha", "first").await.expect("polled");
    f.setup.engine.poll_quota("alpha", "second").await.expect("polled");
    assert_eq!(served(&cold(&f, "ak_1", "hello").await), "alpha/second");
}

#[tokio::test]
async fn d_006_2_member_order_decides_nothing() {
    // `x` is the first declared member, `y` the second; `y`'s window expires with more unspent.
    let f = Fleet::new()
        .provider("x", TOKENS, &[("5h", "tokens")])
        .provider("y", TOKENS, &[("5h", "tokens")])
        .account("x", "a", 1.0)
        .account("y", "a", 1.0)
        .unified(&[("x", "m1"), ("y", "m1")])
        .build()
        .await;
    f.quota.set("x/a", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H * 5 / 2)).used(500_000.0)]);
    f.quota.set("y/a", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset_in(H)).used(200_000.0)]);
    f.setup.engine.poll_quota("x", "a").await.expect("polled");
    f.setup.engine.poll_quota("y", "a").await.expect("polled");
    assert_eq!(served(&cold(&f, "ak_1", "hello").await), "y/a");
}

#[tokio::test]
async fn d_006_3_equal_accounts_alternate_instead_of_sticking() {
    let f = two_tokens().build().await;
    let mut seen = Vec::new();
    for i in 0..4 {
        seen.push(served(&cold(&f, &format!("ak_{i}"), &format!("q {i}")).await));
    }
    // Equal state: the operator's order breaks the first tie, then the deficit alternates them.
    assert_eq!(seen, ["alpha/a", "alpha/b", "alpha/a", "alpha/b"]);
}

#[tokio::test]
async fn equal_state_follows_the_operators_order_like_fill_first() {
    let f = two_tokens().build().await;
    let r = cold(&f, "ak_1", "hello").await;
    assert_eq!(served(&r), "alpha/a");
    let d = r.decision.unwrap();
    assert_eq!(d.candidates[0].share, d.candidates[1].share);
    assert_eq!(d.candidates[0].deficit_before, d.candidates[1].deficit_before);
}
