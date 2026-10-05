//! The decision a record keeps explains every placement (spec 006, US4, T052): a request that
//! failed over shows both attempts with their reasons, rows, latency and usage; a moved warm
//! request names the warm account and why it left; and for every cold or overflow record the
//! served account is the one the recorded deficits and tie order choose (SC-006).

mod common;

use common::*;
use nullrouter_engine::classify;
use nullrouter_engine::records::{AttemptOutcome, RequestRecord};
use nullrouter_engine::routing::{CandidateRow, DecisionKind, MovedBecause, PlacementReason, Tier};
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

const PAYG: &str = "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.price]]\ninput = 2.0\noutput = 8.0\n";

fn body(text: &str) -> Value {
    json!({"model": "u", "stream": false, "messages": [
        {"role": "system", "content": "You are a careful assistant."},
        {"role": "user", "content": text},
    ]})
}

async fn ask(f: &FleetSetup, agent: &str, text: &str) -> RequestRecord {
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

fn fleet() -> Fleet {
    Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .provider("pay", PAYG, &[])
        .account("alpha", "a", 1.0)
        .account("alpha", "b", 1.0)
        .account("pay", "key", 1.0)
        .unified(&[("alpha", "m1"), ("pay", "m1")])
}

#[tokio::test]
async fn a_failed_first_attempt_and_the_success_after_it_both_say_why_they_were_placed() {
    let f = fleet().build().await;
    // Whichever subscription is placed first fails; same-account retries are slice 003's.
    f.setup.mock.push([err(500), err(500), ok()]);
    let req = request(&f.setup, "openai-chat", "u", body("hello"), "ak_1", CancellationToken::new());
    let id = req.id.clone();
    f.setup.engine.text(f.setup.engine.snapshot(), req).await.expect("another account serves");
    let r = settled(&f.setup, &id).await;

    let placed: Vec<_> = r.attempts.iter().filter(|a| a.placement.is_some()).collect();
    assert!(placed.len() >= 2, "{:#?}", r.attempts);
    let first = placed[0];
    assert_eq!(first.placement.unwrap().reason, PlacementReason::ColdByDeficit);
    assert_eq!(first.placement.unwrap().rank, 0);
    assert!(matches!(first.outcome, Some(AttemptOutcome::Failed { .. })), "{:?}", first.outcome);
    assert!(first.ended.is_some(), "a failed attempt has its latency");
    let last = placed.last().unwrap();
    assert!(matches!(last.outcome, Some(AttemptOutcome::Ok)), "{:?}", last.outcome);
    assert!(last.usage.is_some(), "the served attempt keeps its usage");
    assert!(last.ended.is_some());
    // The decision keeps every candidate, with the reason each was or wasn't an option.
    let d = r.decision.expect("a decision");
    assert_eq!(d.candidates.len(), 3);
    assert!(d.candidates.iter().all(|c| c.eligible && c.deficit_before.is_some()));
    assert_eq!(d.candidates.iter().filter(|c| c.tier == Tier::Payg).count(), 1);
    assert_eq!(d.order.len(), 3);
}

#[tokio::test]
async fn a_moved_warm_request_names_the_warm_account_and_why_it_left() {
    let f = fleet().build().await;
    let first = ask(&f, "ak_1", "one long conversation").await;
    let home = served(&first);
    // The account holding the cache is rate limited; the same agent sends the same prompt again.
    let (provider, account) = home.split_once('/').unwrap();
    f.setup.engine.cooldowns.fail(provider, account, "m1", &classify::upstream(429, "slow down"));
    let second = ask(&f, "ak_1", "one long conversation").await;
    let d = second.decision.clone().expect("a decision");
    let warm = d.warm.expect("the warm hit is recorded");
    assert_eq!(format!("{}/{}", warm.provider, warm.account), home);
    assert!(!warm.stayed);
    assert_eq!(warm.moved_because, Some(MovedBecause::RateLimited));
    assert_ne!(served(&second), home);
    assert_eq!(second.attempts.iter().find_map(|a| a.placement).unwrap().reason, PlacementReason::MovedForCapacity);
}

/// The account the recorded rows choose for the first attempt: among the eligible rows of the
/// tier cold work went to, the largest deficit, then the higher share, then the operator's order.
fn recomputed(d: &nullrouter_engine::routing::Decision) -> String {
    let tier =
        if d.candidates.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0) {
            Tier::Subscription
        } else {
            Tier::Payg
        };
    let mut rows: Vec<(usize, &CandidateRow)> =
        d.candidates.iter().enumerate().filter(|(_, c)| c.eligible && c.tier == tier).collect();
    // Candidates are listed in the operator's order, so the index is the order.
    rows.sort_by(|(i, a), (j, b)| {
        let (da, db) = (a.deficit_before.unwrap_or(0), b.deficit_before.unwrap_or(0));
        db.cmp(&da).then(b.share.unwrap_or(0.0).total_cmp(&a.share.unwrap_or(0.0))).then(i.cmp(j))
    });
    let (_, c) = rows[0];
    format!("{}/{}", c.provider, c.account)
}

#[tokio::test]
async fn every_cold_and_overflow_record_is_recomputable_from_its_rows() {
    let f = fleet().build().await;
    let mut records = Vec::new();
    for i in 0..10 {
        records.push(ask(&f, &format!("ak_{i}"), &format!("question {i}")).await);
    }
    // Both subscriptions below their floor: the rest is overflow.
    for a in ["a", "b"] {
        f.quota.set(
            &format!("alpha/{a}"),
            vec![SimWindow::new("5h", "tokens", 1_000_000.0, "2099-01-01T00:00:00Z").used(990_000.0)],
        );
        f.setup.engine.poll_quota("alpha", a).await.expect("polled");
    }
    for i in 10..14 {
        records.push(ask(&f, &format!("ak_{i}"), &format!("question {i}")).await);
    }
    let mut overflow = 0;
    for r in &records {
        let d = r.decision.clone().expect("a decision");
        assert!(matches!(d.kind, DecisionKind::Cold | DecisionKind::Overflow), "{:?}", d.kind);
        overflow += usize::from(d.kind == DecisionKind::Overflow);
        let first = r.attempts.iter().find(|a| a.placement.is_some()).expect("a placed attempt");
        assert_eq!(format!("{}/{}", first.provider, first.account.as_deref().unwrap_or("")), recomputed(&d));
    }
    assert_eq!(overflow, 4);
}
