//! Spec 011 T024, US2 scenarios 1–5, SC-002: routing leaves out a pair only when it is BROKEN.
//! A BROKEN pair is a recorded skip; every other pair, UNKNOWN and untested included, routes as
//! before; client traffic never changes a verdict.

mod common;

use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass};
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_engine::verdict::{Basis, Pair, Rejection, Source, State, Verdict};

fn verdict(state: State) -> Verdict {
    Verdict {
        state,
        reason: "404: model m1 does not exist".into(),
        rejection: (state == State::Broken).then_some(Rejection::ModelNotFound),
        source: Source::Test,
        at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_400_000),
        record: Some("rq_1".into()),
        step: (state == State::Unknown).then_some(0),
        next: None,
        basis: Basis { secret: None, signed_in_at: None, plugin: "sha256:00".into() },
        note: None,
    }
}

async fn two_accounts() -> Setup {
    let plugin = |m: &MockUpstream| {
        // A second model, so a verdict on m1 can be shown to leave m2 alone.
        format!("{}[[models]]\nid = \"m2\"\n", chat_plugin(m, "alpha", ""))
    };
    setup(|m| vec![("alpha", plugin(m))], &[("alpha", "a"), ("alpha", "b")], "").await
}

#[tokio::test]
async fn a_broken_pair_is_skipped_and_the_next_account_serves() {
    let s = two_accounts().await;
    s.engine.verdicts.set(Pair::new("alpha", "a", "m1"), verdict(State::Broken));
    s.mock.on("/alpha", [ok(), ok()]);

    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok(), "{:?}", res.err());
    assert_eq!(accounts_hit(&s), ["alpha-b"]);
    let r = settled(&s, &id).await;
    let skip = r.attempts.iter().find(|a| a.kind == AttemptKind::Skipped).expect("a recorded skip");
    assert_eq!(skip.account.as_deref(), Some("a"));
    let Some(AttemptOutcome::Skipped { reason, class }) = &skip.outcome else { panic!("{skip:?}") };
    assert!(reason.starts_with("BROKEN since 2026-"), "{reason}");
    assert!(reason.ends_with(": 404: model m1 does not exist"), "{reason}");
    assert_eq!(*class, Some(ErrorClass::Broken));

    // Scenario 2: the verdict is per model; m2 on account a still serves.
    let (_, res) = send(&s, "alpha/m2").await;
    assert!(res.is_ok());
    assert_eq!(accounts_hit(&s), ["alpha-b", "alpha-a"]);
}

#[tokio::test]
async fn unknown_and_untested_pairs_route_as_before() {
    let s = two_accounts().await;
    s.engine.verdicts.set(Pair::new("alpha", "a", "m1"), verdict(State::Unknown));
    s.mock.on("/alpha", [ok()]);
    let (_, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    assert_eq!(accounts_hit(&s), ["alpha-a"], "UNKNOWN routes like untested");
}

#[tokio::test]
async fn every_pair_broken_fails_before_any_call() {
    let s = two_accounts().await;
    for a in ["a", "b"] {
        s.engine.verdicts.set(Pair::new("alpha", a, "m1"), verdict(State::Broken));
    }
    let (id, res) = send(&s, "alpha/m1").await;
    let f = res.err().expect("no account to serve");
    assert_eq!(f.status, 503);
    assert!(f.message.contains("alpha/m1 is BROKEN on every account"), "{}", f.message);
    assert_eq!(f.tried.len(), 2, "{:?}", f.tried);
    assert!(f.tried.iter().all(|t| t.reason.starts_with("BROKEN since")), "{:?}", f.tried);
    assert!(s.mock.received().is_empty(), "FR-011: no upstream call");
    let r = settled(&s, &id).await;
    assert!(r.attempts.iter().all(|a| a.kind == AttemptKind::Skipped));
}

#[tokio::test]
async fn client_traffic_never_changes_a_verdict() {
    let s = two_accounts().await;
    s.engine.verdicts.set(Pair::new("alpha", "b", "m1"), verdict(State::Unknown));
    s.mock.on(
        "/alpha",
        [
            Step::json(404, serde_json::json!({"error": {"message": "The model 'm1' does not exist"}})),
            Step::json(404, serde_json::json!({"error": {"message": "The model 'm1' does not exist"}})),
        ],
    );
    let (_, res) = send(&s, "alpha/m1").await;
    assert!(res.is_err());
    assert_eq!(s.mock.received().len(), 2, "both accounts were tried");
    assert_eq!(s.engine.verdicts.get(&Pair::new("alpha", "a", "m1")), None, "FR-010: no verdict from a client");
    assert_eq!(s.engine.verdicts.get(&Pair::new("alpha", "b", "m1")).map(|v| v.state), Some(State::Unknown));
}
