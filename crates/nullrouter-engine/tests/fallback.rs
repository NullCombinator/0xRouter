//! The fallback order (T064, SC-002): same account → other account → other member, then an
//! informational failure. Retries are switched off by override so each failure kind is
//! one request.
//!
//! A member whose provider isn't installed can't reach a plan: the registry drops or
//! refuses the whole unified model at load, so only the no-account skip is tested here.

mod common;

use common::*;
use nullrouter_engine::records::{AttemptKind, AttemptOutcome, Outcome};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 500 = { retries = 0 }, 502 = { retries = 0 }, 503 = { retries = 0 }, 504 = { retries = 0 } }";

async fn two_by_two() -> Setup {
    setup(
        |m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY)), ("beta", chat_plugin(m, "beta", NO_RETRY))],
        &[("alpha", "main"), ("alpha", "backup"), ("beta", "main")],
        &unified(&[("alpha", "m1"), ("beta", "m1")]),
    )
    .await
}

#[tokio::test]
async fn every_failure_kind_falls_back_account_then_member() {
    for code in [429, 500, 502, 503, 504, 401, 403, 404] {
        let s = two_by_two().await;
        s.mock.on("/alpha", [err(code), err(code)]);
        s.mock.on("/beta", [ok()]);
        let (id, res) = send(&s, "u").await;
        assert!(res.is_ok(), "{code}");
        assert_eq!(accounts_hit(&s), ["alpha-main", "alpha-backup", "beta-main"], "{code}");
        let r = s.engine.records.get(&id).unwrap();
        assert_eq!(
            trail(&r),
            [
                t("alpha", "main", AttemptKind::Initial),
                t("alpha", "backup", AttemptKind::NextAccount),
                t("beta", "main", AttemptKind::NextMember)
            ],
            "{code}"
        );
        assert_eq!(r.unified_model.as_deref(), Some("u"));
        assert_eq!(r.served_by.unwrap().provider, "beta");
    }
}

#[tokio::test]
async fn when_everything_fails_the_client_gets_503_with_every_attempt() {
    let s = two_by_two().await;
    s.mock.push([err(500), err(502), err(503)]);
    let (id, res) = send(&s, "u").await;
    let f = res.err().expect("all failed");
    assert_eq!(f.status, 503);
    assert!(f.message.starts_with(&format!("0router: no provider could serve u (record {id})")), "{}", f.message);
    let lines: Vec<_> = f.tried.iter().map(|t| (t.provider.as_str(), t.account.as_deref(), t.status)).collect();
    assert_eq!(
        lines,
        [("alpha", Some("main"), Some(500)), ("alpha", Some("backup"), Some(502)), ("beta", Some("main"), Some(503))]
    );
    assert!(f.message.contains("alpha/backup m1: 502"), "{}", f.message);
    let ra = f.retry_after.expect("retry-after from the earliest cooldown");
    assert!((1..=30).contains(&ra), "{ra}");
    assert_eq!(s.engine.records.get(&id).unwrap().outcome, Outcome::Failed);
}

#[tokio::test]
async fn a_direct_target_stops_after_its_accounts() {
    let s = two_by_two().await;
    s.mock.on("/alpha", [err(500), err(500)]);
    s.mock.on("/beta", [ok()]);
    let (_, res) = send(&s, "alpha/m1").await;
    assert_eq!(res.err().expect("no other member").status, 503);
    assert!(paths(&s).iter().all(|p| p.starts_with("/alpha")), "{:?}", paths(&s));
    assert_eq!(s.mock.received().len(), 2);
}

#[tokio::test]
async fn a_request_error_comes_back_at_once_with_the_upstream_message_first() {
    let s = two_by_two().await;
    s.mock.push([serde_step(400, "max_tokens is too large"), ok()]);
    let (id, res) = send(&s, "u").await;
    let f = res.err().expect("no fallback on 400");
    assert_eq!(f.status, 400);
    assert!(f.message.starts_with(&format!("max_tokens is too large (record {id})")), "{}", f.message);
    assert_eq!(s.mock.received().len(), 1);
}

fn serde_step(status: u16, message: &str) -> nullrouter_engine::testkit::Step {
    nullrouter_engine::testkit::Step::json(
        status,
        json!({"error": {"message": message, "type": "invalid_request_error"}}),
    )
}

#[tokio::test]
async fn a_member_without_an_account_is_a_skipped_attempt() {
    let s = setup(
        |m| vec![("gamma", chat_plugin(m, "gamma", "")), ("alpha", chat_plugin(m, "alpha", ""))],
        &[("alpha", "main")],
        &unified(&[("gamma", "m1"), ("alpha", "m1")]),
    )
    .await;
    s.mock.push([ok()]);
    let (id, res) = send(&s, "u").await;
    assert!(res.is_ok());
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(r.attempts[0].kind, AttemptKind::Skipped);
    assert_eq!(r.attempts[0].provider, "gamma");
    let Some(AttemptOutcome::Skipped { reason }) = &r.attempts[0].outcome else {
        panic!("{:?}", r.attempts[0].outcome)
    };
    assert!(reason.contains("no enabled account"), "{reason}");
    assert_eq!(paths(&s), ["/alpha/chat/completions"]);
}

#[tokio::test]
async fn a_member_that_cant_carry_the_request_is_skipped() {
    let s = setup(
        |m| vec![("msgs", messages_plugin(m, "msgs")), ("alpha", chat_plugin(m, "alpha", ""))],
        &[("msgs", "main"), ("alpha", "main")],
        &unified(&[("msgs", "m1"), ("alpha", "m1")]),
    )
    .await;
    s.mock.push([ok()]);
    let body = json!({"model": "u", "messages": [{"role": "user", "content": "hi"}], "response_format": {"type": "json_object"}});
    let req = request(&s, "openai-chat", "u", body, "ak_test", CancellationToken::new());
    let id = req.id.clone();
    assert!(s.engine.text(s.engine.snapshot(), req).await.is_ok());
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!((r.attempts[0].provider.as_str(), r.attempts[0].kind), ("msgs", AttemptKind::Skipped));
    let Some(AttemptOutcome::Skipped { reason }) = &r.attempts[0].outcome else {
        panic!("{:?}", r.attempts[0].outcome)
    };
    assert!(reason.contains("response format"), "{reason}");
    assert_eq!(paths(&s), ["/alpha/chat/completions"], "nothing reached the member that can't carry it");
}
