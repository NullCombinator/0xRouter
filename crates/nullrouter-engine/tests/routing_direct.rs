//! A direct `provider/model` target with several accounts is spread like a unified model of the
//! same accounts (spec 006, US8, T086): the same decisions for the same warm and cold sequence,
//! a place in the routing view, and no size check at request time (FR-032).

mod common;

use common::*;
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::route;
use nullrouter_engine::routing::PlacementReason;
use nullrouter_engine::testkit::Step;
use serde_json::{Value, json};
use std::time::SystemTime;
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

const SYSTEM: &str = "You are a careful assistant. Answer in one short sentence and never guess.";

fn body(target: &str, turns: &[&str]) -> Value {
    let mut messages = vec![json!({"role": "system", "content": SYSTEM})];
    for (i, t) in turns.iter().enumerate() {
        messages.push(json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": t}));
    }
    json!({"model": target, "stream": false, "messages": messages})
}

/// Three accounts of one provider; `unified` also declares the model `u` over them.
fn three(unified: bool) -> Fleet {
    let f = Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .account("alpha", "one", 1.0)
        .account("alpha", "two", 1.0)
        .account("alpha", "three", 1.0);
    if unified { f.unified(&[("alpha", "m1")]) } else { f }
}

async fn turn(f: &FleetSetup, target: &str, agent: &str, turns: &[&str]) -> Result<RequestRecord, String> {
    let req = request(&f.setup, "openai-chat", target, body(target, turns), agent, CancellationToken::new());
    let id = req.id.clone();
    f.setup.engine.text(f.setup.engine.snapshot(), req).await.map_err(|e| format!("{e:?}"))?;
    Ok(settled(&f.setup, &id).await)
}

/// Who served and why.
fn placed(r: &RequestRecord) -> (String, PlacementReason) {
    let s = r.served_by.as_ref().expect("served");
    let reason = r.attempts.last().and_then(|a| a.placement).expect("a placement").reason;
    (format!("{}/{}", s.provider, s.account.as_deref().unwrap_or("")), reason)
}

/// A fixed sequence: cold agents, their follow-ups (warm), a new cold agent, a longer follow-up.
const SEQUENCE: &[(&str, &[&str])] = &[
    ("ak_a", &["one"]),
    ("ak_b", &["alpha"]),
    ("ak_c", &["red"]),
    ("ak_a", &["one", "two", "three"]),
    ("ak_b", &["alpha", "beta", "gamma"]),
    ("ak_d", &["north"]),
    ("ak_c", &["red", "green", "blue"]),
    ("ak_a", &["one", "two", "three", "four", "five"]),
];

async fn run(f: &FleetSetup, target: &str) -> Vec<(String, PlacementReason)> {
    let mut out = Vec::new();
    for (agent, turns) in SEQUENCE {
        out.push(placed(&turn(f, target, agent, turns).await.expect("served")));
    }
    out
}

#[tokio::test]
async fn a_direct_target_places_like_a_unified_model_of_the_same_accounts() {
    let unified = three(true).build().await;
    let direct = three(false).build().await;
    let via_unified = run(&unified, "u").await;
    let via_direct = run(&direct, "alpha/m1").await;
    assert_eq!(via_direct, via_unified);

    // The sequence did spread cold work and keep warm work: three agents landed on three
    // accounts, and each follow-up stayed where its prefix was.
    let cold: Vec<_> = via_direct.iter().filter(|(_, r)| *r == PlacementReason::ColdByDeficit).collect();
    let mut accounts: Vec<_> = cold.iter().map(|(a, _)| a.as_str()).collect();
    accounts.sort_unstable();
    accounts.dedup();
    assert_eq!(accounts.len(), 3, "{via_direct:?}");
    assert_eq!(via_direct[3].0, via_direct[0].0);
    assert_eq!(via_direct[4].0, via_direct[1].0);
    assert_eq!(via_direct[7].0, via_direct[0].0);
    assert!(via_direct[3..5].iter().all(|(_, r)| *r == PlacementReason::Warm), "{via_direct:?}");
}

#[tokio::test]
async fn the_routing_view_lists_a_direct_target_once_it_has_seen_cold_work() {
    let f = three(false).build().await;
    let st = f.setup.engine.snapshot();
    // Nothing placed yet: a direct target has no ledger, so the unfiltered view leaves it out.
    assert!(route::view_targets(&f.setup.engine, &st, None).is_empty());

    turn(&f, "alpha/m1", "ak_a", &["one"]).await.expect("served");
    let st = f.setup.engine.snapshot();
    assert_eq!(route::view_targets(&f.setup.engine, &st, None), ["alpha/m1"]);
    let views = route::view_all(&f.setup.engine, &st, None, SystemTime::now());
    let [view] = views.as_slice() else { panic!("{views:?}") };
    assert_eq!(view.target, "alpha/m1");
    let mut names: Vec<_> = view.accounts.iter().map(|a| a.account.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["one", "three", "two"]);
    assert!(view.accounts.iter().all(|a| a.share.is_some()), "{view:?}");
    // The account that took the request is the one that is no longer owed.
    let owed = |name: &str| view.accounts.iter().find(|a| a.account == name).unwrap().deficit;
    assert!(owed("one") < owed("two"), "one served: {view:?}");
}

#[tokio::test]
async fn a_member_that_refuses_a_request_for_its_size_is_moved_past_by_ordinary_retry() {
    let f = three(true).build().await;
    // No size check precedes the attempt: the first member sees the request, refuses it in
    // words the ordinary classifier treats as a reason to move on, and the next serves it.
    f.setup.mock.push([Step::json(
        400,
        json!({"error": {"message": "the prompt is beyond this model's capacity", "type": "invalid_request_error"}}),
    )]);
    let r = turn(&f, "u", "ak_a", &["one"]).await.expect("served after the refusal");
    assert_eq!(r.attempts.len(), 2, "{:?}", r.attempts);
    // The same account's retry budget comes first (R7); either way the client sees an answer.
    assert_eq!(r.served_by.as_ref().unwrap().account, r.attempts.last().unwrap().account);
}

#[tokio::test]
async fn a_plain_client_error_is_not_retried_elsewhere() {
    let f = three(true).build().await;
    f.setup.mock.push([Step::json(400, json!({"error": {"message": "bad field", "type": "invalid_request_error"}}))]);
    let err = turn(&f, "u", "ak_a", &["one"]).await.expect_err("a client error reaches the client");
    assert!(err.contains("400") || err.to_lowercase().contains("bad field"), "{err}");
}
