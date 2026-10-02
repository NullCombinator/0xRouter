//! Stay-warm (T065, SC-003, research R8): an agent keeps the account that last served it
//! until that account fails, and a success is only counted when the answer completes.

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::Answer;
use zerorouter_engine::classify;
use zerorouter_engine::keys::AgentId;
use zerorouter_engine::plan::Warm;
use zerorouter_engine::records::AttemptKind;
use zerorouter_engine::testkit::Step;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 500 = { retries = 0 } }";

fn agent() -> AgentId {
    AgentId::new("ak_test", Some("sess-1"))
}

fn warm(provider: &str, account: &str) -> Option<Warm> {
    Some(Warm { provider: provider.into(), account: Some(account.into()) })
}

#[tokio::test]
async fn the_agent_stays_on_backup_until_backup_fails_then_returns_to_main() {
    let s =
        setup(|m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY))], &[("alpha", "main"), ("alpha", "backup")], "")
            .await;
    // A 429 rests main for 2 s (backoff level 1).
    s.mock.push([Step::json(429, json!({"error": {"message": "slow down"}})), ok()]);
    assert!(send(&s, "alpha/m1").await.1.is_ok());
    assert_eq!(accounts_hit(&s), ["alpha-main", "alpha-backup"]);
    assert_eq!(s.engine.warm.get(&agent(), "alpha/m1"), warm("alpha", "backup"));

    s.mock.push([ok()]);
    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    assert_eq!(accounts_hit(&s)[2], "alpha-backup", "warm backup first, though main is first in operator order");
    assert_eq!(trail(&s.engine.records.get(&id).unwrap()), [t("alpha", "backup", AttemptKind::Initial)]);

    tokio::time::sleep(Duration::from_millis(2100)).await;
    s.mock.push([Step::json(429, json!({"error": {"message": "slow down"}})), ok()]);
    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    assert_eq!(&accounts_hit(&s)[3..], ["alpha-backup", "alpha-main"]);
    assert_eq!(
        trail(&s.engine.records.get(&id).unwrap()),
        [t("alpha", "backup", AttemptKind::Initial), t("alpha", "main", AttemptKind::NextAccount)]
    );
    assert_eq!(s.engine.warm.get(&agent(), "alpha/m1"), warm("alpha", "main"));

    s.mock.push([ok()]);
    assert!(send(&s, "alpha/m1").await.1.is_ok());
    assert_eq!(accounts_hit(&s)[5], "alpha-main");
}

#[tokio::test]
async fn a_cooling_warm_account_is_skipped() {
    let s =
        setup(|m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY))], &[("alpha", "main"), ("alpha", "backup")], "")
            .await;
    s.engine.warm.set(&agent(), "alpha/m1", Warm { provider: "alpha".into(), account: Some("backup".into()) });
    s.engine.cooldowns.fail("alpha", "backup", "m1", &classify::upstream(500, "boom"));
    s.mock.push([ok()]);
    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    assert_eq!(accounts_hit(&s), ["alpha-main"]);
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(trail(&r), [t("alpha", "backup", AttemptKind::Skipped), t("alpha", "main", AttemptKind::Initial)]);
}

#[tokio::test]
async fn with_two_requests_in_flight_the_last_success_wins() {
    let s = setup(
        |m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY)), ("beta", chat_plugin(m, "beta", NO_RETRY))],
        &[("alpha", "main"), ("beta", "main")],
        &unified(&[("alpha", "m1"), ("beta", "m1")]),
    )
    .await;
    let Step::Stream { status, headers, frames, cut, .. } = chat_chunks() else { unreachable!() };
    let slow = Step::Stream { status, headers, frames, every: Duration::from_millis(200), cut };
    s.mock.on("/alpha", [slow, err(500)]);
    s.mock.on("/beta", [chat_chunks()]);

    let a = request(&s, "openai-chat", "u", chat_body("u", true), "ak_test", CancellationToken::new());
    let Answer::Events { rx: mut rx_a, .. } = s.engine.text(s.engine.snapshot(), a).await.unwrap() else {
        panic!("events")
    };
    let b = request(&s, "openai-chat", "u", chat_body("u", true), "ak_test", CancellationToken::new());
    let Answer::Events { rx: mut rx_b, .. } = s.engine.text(s.engine.snapshot(), b).await.unwrap() else {
        panic!("events")
    };
    drain(&mut rx_b).await;
    assert_eq!(s.engine.warm.get(&agent(), "u"), warm("beta", "main"), "b finished first, on beta");
    drain(&mut rx_a).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(s.engine.warm.get(&agent(), "u"), warm("alpha", "main"), "a finished last, on alpha");
}

#[tokio::test]
async fn a_stream_that_breaks_isnt_counted_as_warm() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY))], &[("alpha", "main")], "").await;
    // Frames spaced out, so the cut can't overtake them.
    let Step::Stream { status, headers, frames, .. } = chat_chunks().cut_after(2) else { unreachable!() };
    s.mock.push([Step::Stream { status, headers, frames, every: Duration::from_millis(20), cut: true }]);
    let req = request(&s, "openai-chat", "alpha/m1", chat_body("alpha/m1", true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    drain(&mut rx).await;
    settled(&s, &id).await;
    assert_eq!(s.engine.warm.get(&agent(), "alpha/m1"), None);
}
