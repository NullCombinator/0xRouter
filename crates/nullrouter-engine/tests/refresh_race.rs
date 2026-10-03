//! A refresh racing a sign-in, and a refresh off the token's hosts (security review M1, M2).
//!
//! M1: a CLI sign-in written to `tokens.toml` while a refresh of the old grant is in flight
//! survives that refresh, in the file and in the cell, whether the refresh succeeds or fails
//! for good. M2: the refresh token goes only to a host the account's tokens are bound to.

mod common;
mod signin_kit;

use std::time::Duration;

use nullrouter_engine::signin::refresh::Refreshed;
use nullrouter_engine::testkit::Failure;
use nullrouter_engine::tokens::{self, AccountState, PersistedState};
use nullrouter_registry::SecretString;
use signin_kit::{Shape, kit};

const HOUR: Duration = Duration::from_secs(3600);

/// Writes a new grant for `p/a` to `tokens.toml` as the CLI's sign-in does, without the
/// server reloading (the reload over the socket failed). Returns its access token.
fn cli_sign_in(k: &signin_kit::Kit) -> String {
    let (access, refresh, _) = k.idp.grant();
    let mut e = k.stored("p", "a");
    e.access_token = SecretString::new(access.clone());
    e.refresh_token = Some(SecretString::new(refresh));
    e.state = None;
    e.state_since = None;
    e.state_reason = None;
    tokens::update(k.home(), "p", "a", |slot| *slot = Some(e)).unwrap();
    access
}

#[tokio::test]
async fn a_sign_in_during_a_successful_refresh_is_not_reverted() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    k.sign_in("p", "a", |_| {});
    let old = k.access("p", "a");
    k.idp.fail_token([Failure::Slow(Duration::from_millis(600))]);
    let engine = k.engine().clone();
    let refreshing = tokio::spawn(async move { engine.refresh_account("p", "a").await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let signed_in = cli_sign_in(&k);

    assert_eq!(refreshing.await.unwrap(), Refreshed::Fresh);
    assert_eq!(k.idp.refresh_calls(), 1, "the old grant was refreshed");
    assert!(k.stored("p", "a").access_token.matches(&signed_in), "the file keeps the sign-in");
    assert_eq!(k.access("p", "a"), signed_in, "the cell follows the file, not the refresh");
    assert_ne!(k.access("p", "a"), old);
    let v = k.engine().tokens.get("p", "a").unwrap();
    assert_eq!(v.state, AccountState::Active);
    assert_eq!(k.engine().snapshot().redactor.redact(&signed_in), "***");
}

#[tokio::test]
async fn an_old_grants_permanent_failure_never_marks_a_fresh_sign_in() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    // A refresh token the identity provider doesn't know: the refresh fails for good.
    k.sign_in("p", "a", |e| e.refresh_token = Some(SecretString::new("revoked-refresh-SENTINEL")));
    k.idp.fail_token([Failure::Slow(Duration::from_millis(600))]);
    let engine = k.engine().clone();
    let refreshing = tokio::spawn(async move { engine.refresh_account("p", "a").await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let signed_in = cli_sign_in(&k);

    assert!(matches!(refreshing.await.unwrap(), Refreshed::Permanent(_)));
    let stored = k.stored("p", "a");
    assert!(stored.access_token.matches(&signed_in));
    assert_eq!(stored.state, None, "the fresh sign-in is not marked needs_sign_in");
    assert_eq!(k.access("p", "a"), signed_in, "the cell takes the stored sign-in");
    assert_eq!(k.engine().tokens.get("p", "a").unwrap().state, AccountState::Active);
}

#[tokio::test]
async fn a_token_url_off_the_bound_hosts_gets_no_refresh_token() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    // Bound somewhere else: the plugin's token URL (127.0.0.1) is not one of its hosts.
    k.sign_in("p", "a", |e| e.hosts = ["auth.elsewhere.example".to_owned()].into());
    let Refreshed::Permanent(reason) = k.engine().refresh_account("p", "a").await else { panic!("permanent") };
    assert!(reason.contains("127.0.0.1") && reason.contains("nullrouter accounts signin p a"), "{reason}");
    assert_eq!(k.idp.token_calls(), 0, "nothing was sent");
    let stored = k.stored("p", "a");
    assert_eq!(stored.state, Some(PersistedState::NeedsSignIn));
    assert!(stored.state_reason.unwrap().contains("not one this account's sign-in covered"));
    assert!(matches!(k.engine().tokens.get("p", "a").unwrap().state, AccountState::NeedsSignIn { .. }));
}
