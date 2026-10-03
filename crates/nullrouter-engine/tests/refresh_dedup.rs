//! Refresh dedup and transient failures (spec 005 T047, FR-012, FR-013, research R10):
//! concurrent requests at expiry share one refresh; a transient failure while the token is
//! valid keeps the account `Active` and is retried; once the token has expired the account
//! is `Refreshing`, requests fall back, and it returns to `Active` on success.

mod common;
mod signin_kit;

use std::time::{Duration, SystemTime};

use common::{send, send_as, settled};
use nullrouter_engine::accounts;
use nullrouter_engine::maintenance;
use nullrouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass};
use nullrouter_engine::signin::refresh::Timing;
use nullrouter_engine::testkit::Failure;
use nullrouter_engine::tokens::AccountState;
use signin_kit::{Shape, bearer, eventually, kit};
use tokio_util::sync::CancellationToken;

const HOUR: Duration = Duration::from_secs(3600);

fn upkeep(k: &signin_kit::Kit) -> CancellationToken {
    let stop = CancellationToken::new();
    let s = stop.clone();
    maintenance::spawn(k.engine().clone(), async move { s.cancelled().await });
    stop
}

fn quick() -> Timing {
    Timing { backoff: vec![Duration::from_millis(300), Duration::from_millis(600)], ..Timing::default() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_concurrent_requests_at_expiry_cause_one_refresh() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    k.sign_in("p", "a", |e| {
        e.expires_at = SystemTime::now() + Duration::from_secs(5);
        e.signed_in_at = SystemTime::now() - HOUR;
    });
    let old = k.access("p", "a");
    let s = std::sync::Arc::new(k);
    let tasks: Vec<_> = (0..50)
        .map(|_| {
            let s = s.clone();
            tokio::spawn(async move { send(&s.s, "p/m1").await.1.is_ok() })
        })
        .collect();
    for t in tasks {
        assert!(t.await.unwrap(), "every request served");
    }
    assert_eq!(s.idp.refresh_calls(), 1, "merged into one refresh (FR-012)");
    let got = s.s.mock.received();
    assert_eq!(got.len(), 50);
    let new = s.access("p", "a");
    assert!(got.iter().all(|r| bearer(r) == new), "every request used the new token");
    assert_ne!(new, old);
}

#[tokio::test]
async fn a_transient_failure_while_the_token_is_valid_keeps_the_account_active_and_retries() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    k.engine().refresher.set_timing(quick());
    // Due for refresh now (within the 30 min lead), valid for another 20 min.
    k.sign_in("p", "a", |e| {
        e.expires_at = SystemTime::now() + Duration::from_secs(20 * 60);
        e.signed_in_at = SystemTime::now() - HOUR;
    });
    k.idp.fail_token([Failure::Status(503)]);
    let stop = upkeep(&k);
    assert!(eventually(Duration::from_secs(2), || k.idp.token_calls() >= 1).await);
    assert!(eventually(Duration::from_secs(1), || k.engine().refresher.attempts("p", "a") == 1).await);
    assert_eq!(k.engine().tokens.get("p", "a").unwrap().state, AccountState::Active, "not taken out of service");
    let (id, res) = send(&k.s, "p/m1").await;
    assert!(res.is_ok(), "still serves with its valid token");
    assert_eq!(settled(&k.s, &id).await.attempts.len(), 1);
    // Retried after the backoff, and it succeeds.
    assert!(eventually(Duration::from_secs(3), || k.idp.refresh_calls() == 2).await);
    assert!(eventually(Duration::from_secs(1), || k.engine().refresher.attempts("p", "a") == 0).await);
    stop.cancel();
    assert!(k.stored("p", "a").last_refresh_at.is_some());
}

#[tokio::test]
async fn an_expired_token_whose_refresh_fails_is_refreshing_falls_back_and_recovers() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a", "b"], HOUR).await;
    k.engine().refresher.set_timing(quick());
    k.sign_in("p", "a", |e| {
        e.expires_at = SystemTime::now() - Duration::from_secs(1);
        e.signed_in_at = SystemTime::now() - HOUR;
    });
    k.sign_in("p", "b", |_| {});
    k.idp.fail_token([Failure::Status(503), Failure::Status(502)]);

    // The request's own refresh fails: `a` is skipped as refreshing, `b` serves.
    let (id, res) = send(&k.s, "p/m1").await;
    assert!(res.is_ok());
    let r = settled(&k.s, &id).await;
    assert_eq!(r.attempts[0].kind, AttemptKind::Skipped);
    assert!(
        matches!(&r.attempts[0].outcome, Some(AttemptOutcome::Skipped { class: Some(ErrorClass::TokenRefreshing), reason }) if reason.contains("refresh retrying")),
        "{:?}",
        r.attempts[0].outcome
    );
    assert_eq!(r.served_by.unwrap().account.as_deref(), Some("b"));
    assert!(matches!(k.engine().tokens.get("p", "a").unwrap().state, AccountState::Refreshing { attempts: 1, .. }));
    let st = k.engine().snapshot();
    let a = st.accounts.get("p", "a").unwrap();
    assert_eq!(accounts::out_of_service(a, &k.engine().tokens), Some(accounts::Withheld::Refreshing));

    // Another agent's request (no warm account) skips it at the plan, without another
    // refresh call.
    let (id, res) = send_as(&k.s, "p/m1", "ak_other").await;
    assert!(res.is_ok());
    let r = settled(&k.s, &id).await;
    assert!(
        r.attempts.iter().any(|t| matches!(
            &t.outcome,
            Some(AttemptOutcome::Skipped { class: Some(ErrorClass::TokenRefreshing), .. })
        ))
    );
    assert_eq!(k.idp.token_calls(), 1);

    // Backoff 300 ms, then 600 ms: the second retry succeeds and `a` is active again.
    let stop = upkeep(&k);
    assert!(
        eventually(Duration::from_secs(4), || k.engine().tokens.get("p", "a").unwrap().state == AccountState::Active)
            .await
    );
    stop.cancel();
    assert_eq!(k.idp.refresh_calls(), 3);
    assert_eq!(accounts::out_of_service(a, &k.engine().tokens), None);
    assert!(k.idp.token_valid(&k.access("p", "a")));
}

#[test]
fn the_product_backoff_is_10s_30s_1min_then_every_2min() {
    let t = Timing::default();
    let got: Vec<u64> = (1..=6).map(|n| t.backoff_after(n).as_secs()).collect();
    assert_eq!(got, [10, 30, 60, 120, 120, 120]);
    assert_eq!(t.use_margin, Duration::from_secs(30));
    assert_eq!(t.timeout, Duration::from_secs(15));
}

#[tokio::test]
async fn a_permanent_failure_persists_needs_sign_in() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a", "b"], HOUR).await;
    k.sign_in("p", "a", |e| {
        e.expires_at = SystemTime::now() - Duration::from_secs(1);
        e.signed_in_at = SystemTime::now() - HOUR;
    });
    k.sign_in("p", "b", |_| {});
    k.idp.fail_token([Failure::permanent("invalid_grant")]);
    let (id, res) = send(&k.s, "p/m1").await;
    assert!(res.is_ok());
    let r = settled(&k.s, &id).await;
    assert!(
        matches!(&r.attempts[0].outcome, Some(AttemptOutcome::Skipped { class: Some(ErrorClass::NeedsSignIn), .. })),
        "{:?}",
        r.attempts[0].outcome
    );
    let stored = k.stored("p", "a");
    assert_eq!(stored.state, Some(nullrouter_engine::tokens::PersistedState::NeedsSignIn));
    assert_eq!(stored.state_reason.as_deref(), Some("invalid_grant"));
    assert!(stored.state_since.is_some());
    assert!(matches!(k.engine().tokens.get("p", "a").unwrap().state, AccountState::NeedsSignIn { .. }));
}
