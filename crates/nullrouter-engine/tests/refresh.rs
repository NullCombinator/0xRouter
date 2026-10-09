//! Token refresh (spec 005 T046, FR-010, FR-011, research R9): proactive refresh with no
//! traffic, refresh before use near expiry, refresh-and-retry on a rejected token for every
//! sign-in shape (xai included, R19), rotation, and a running stream keeping its token.

mod common;
mod signin_kit;

use std::time::{Duration, Instant, SystemTime};

use common::{chat_body, drain, request, send, settled};
use nullrouter_engine::attempt::{Answer, Piece};
use nullrouter_engine::maintenance;
use nullrouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass};
use nullrouter_engine::signin::refresh::{Refreshed, Timing};
use nullrouter_engine::testkit::Step;
use nullrouter_wire::ir::Event;
use serde_json::json;
use signin_kit::{Shape, bearer, eventually, kit};
use tokio_util::sync::CancellationToken;

const HOUR: Duration = Duration::from_secs(3600);

/// The maintenance task over the kit's engine; stops when the token is cancelled.
fn upkeep(k: &signin_kit::Kit) -> CancellationToken {
    let stop = CancellationToken::new();
    let s = stop.clone();
    maintenance::spawn(k.engine().clone(), async move { s.cancelled().await });
    stop
}

#[tokio::test]
async fn proactive_refresh_fires_at_expiry_minus_lead_with_no_traffic() {
    // `low`'s lead (1 s) is under the minimum (2 s here, 30 min in the product): 2 s wins.
    // `high`'s lead (10 s) is over half the 6 s lifetime: 3 s wins.
    let k = kit(&[("low", Shape::Device, "1s"), ("high", Shape::Device, "10s")], &["a"], Duration::from_secs(6)).await;
    k.engine().refresher.set_timing(Timing {
        min_lead: Duration::from_secs(2),
        use_margin: Duration::from_millis(100),
        ..Timing::default()
    });
    let start = Instant::now();
    k.sign_in("low", "a", |_| {});
    k.sign_in("high", "a", |_| {});
    let (low, high) = (k.stored("low", "a"), k.stored("high", "a"));
    let first_refresh = |e: &nullrouter_engine::tokens::TokenEntry| {
        let r = e.refresh_token.as_ref().unwrap().with_exposed(str::to_owned);
        move |idp: &nullrouter_engine::testkit::MockIdp| {
            idp.token_requests().iter().any(|t| t.get("refresh_token") == Some(r.as_str()))
        }
    };
    let (low_done, high_done) = (first_refresh(&low), first_refresh(&high));
    let stop = upkeep(&k);

    assert!(eventually(Duration::from_secs(5), || high_done(&k.idp)).await, "high refreshed");
    let high_at = start.elapsed();
    assert!(eventually(Duration::from_secs(5), || low_done(&k.idp)).await, "low refreshed");
    let low_at = start.elapsed();
    stop.cancel();
    // `tokens.toml` keeps whole seconds, so each time may be up to 1 s early.
    assert!(high_at >= Duration::from_millis(2000) && high_at < Duration::from_millis(3600), "high at {high_at:?}");
    assert!(low_at >= Duration::from_millis(3000) && low_at < Duration::from_millis(4600), "low at {low_at:?}");
    assert!(k.s.mock.received().is_empty(), "no traffic");
    // Written to disk, swapped into the cell.
    let old = low.access_token.with_exposed(str::to_owned);
    assert!(eventually(Duration::from_secs(1), || k.stored("low", "a").last_refresh_at.is_some()).await);
    assert!(!k.stored("low", "a").access_token.matches(&old));
    assert!(k.engine().tokens.get("low", "a").unwrap().entry.access_token.matches(&k.access("low", "a")));
}

#[tokio::test]
async fn a_token_near_expiry_is_refreshed_before_use() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    // Expires in 10 s: within the 30 s margin (a long-lived token: no cap applies).
    k.sign_in("p", "a", |e| {
        e.expires_at = SystemTime::now() + Duration::from_secs(10);
        e.signed_in_at = SystemTime::now() - HOUR;
    });
    let old = k.access("p", "a");
    let (id, res) = send(&k.s, "p/m1").await;
    assert!(res.is_ok());
    assert_eq!(k.idp.refresh_calls(), 1);
    let got = k.s.mock.received();
    assert_eq!(got.len(), 1, "one request, with the new token");
    assert_ne!(bearer(&got[0]), old);
    assert_eq!(bearer(&got[0]), k.access("p", "a"));
    assert_eq!(settled(&k.s, &id).await.attempts.len(), 1);

    // Far from expiry: no refresh.
    let (_, res) = send(&k.s, "p/m1").await;
    assert!(res.is_ok());
    assert_eq!(k.idp.refresh_calls(), 1);
}

#[tokio::test]
async fn a_rejected_token_is_refreshed_and_retried_once_on_the_same_account_for_every_shape() {
    for shape in [Shape::Device, Shape::PkceForm, Shape::PkceJson] {
        let k = kit(&[("p", shape, "5m")], &["a", "b"], HOUR).await;
        // Valid by our clock, revoked at the provider.
        k.sign_in("p", "a", |e| e.access_token = nullrouter_registry::SecretString::new("revoked-SENTINEL"));
        k.sign_in("p", "b", |_| {});
        let (id, res) = send(&k.s, "p/m1").await;
        assert!(res.is_ok(), "{shape:?}: the client sees only the answer");
        assert_eq!(k.idp.refresh_calls(), 1, "{shape:?}");
        let sent = k.idp.token_requests().pop().unwrap();
        assert_eq!(sent.json, matches!(shape, Shape::PkceJson), "{shape:?}: the declared body");
        let got = k.s.mock.received();
        assert_eq!(got.len(), 2, "{shape:?}");
        assert_eq!(bearer(&got[0]), "revoked-SENTINEL");
        assert_eq!(bearer(&got[1]), k.access("p", "a"), "{shape:?}: retried on the same account");
        let r = settled(&k.s, &id).await;
        assert_eq!(r.served_by.unwrap().account.as_deref(), Some("a"));
        let kinds: Vec<_> = r.attempts.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, [AttemptKind::Initial, AttemptKind::SameAccountRetry], "{shape:?}: no fallback counted");
        assert!(matches!(
            r.attempts[0].outcome,
            Some(AttemptOutcome::Failed { status: Some(401), class: ErrorClass::Auth, .. })
        ));
        assert!(k.engine().cooldowns.cooling("p", "a", "m1").is_none(), "{shape:?}: no cooldown");
    }
}

#[tokio::test]
async fn a_token_rejected_again_after_refresh_is_not_refreshed_twice() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    k.sign_in("p", "a", |_| {});
    k.s.mock.on("/p", [signin_kit::expired(), signin_kit::expired()]);
    let (_, res) = send(&k.s, "p/m1").await;
    assert!(res.is_err());
    assert_eq!(k.idp.refresh_calls(), 1);
    assert_eq!(k.s.mock.received().len(), 2, "one retry only");
}

#[tokio::test]
async fn rotation_keeps_the_new_refresh_token_or_the_old_one() {
    for rotate in [true, false] {
        let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
        k.idp.set_rotation(rotate);
        k.sign_in("p", "a", |_| {});
        let before = k.stored("p", "a").refresh_token.unwrap().with_exposed(str::to_owned);
        assert_eq!(k.engine().refresh_account("p", "a").await, Refreshed::Fresh);
        let after = k.stored("p", "a");
        assert_eq!(!after.refresh_token.as_ref().unwrap().matches(&before), rotate, "rotate = {rotate}");
        let cell = k.engine().tokens.get("p", "a").unwrap();
        assert!(cell.entry.refresh_token.as_ref().unwrap().with_exposed(|t| after.refresh_token.unwrap().matches(t)));
        // The previous generation is still masked.
        let old = cell.previous.as_ref().unwrap().access.with_exposed(str::to_owned);
        assert_eq!(k.engine().snapshot().redactor.redact(&old), "***");
    }
}

#[tokio::test]
async fn a_refresh_during_a_running_stream_leaves_it_on_its_token() {
    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    k.sign_in("p", "a", |_| {});
    let old = k.access("p", "a");
    let frames = (0..6)
        .map(|i| {
            let v = json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": {"content": format!("w{i} ")}, "finish_reason": null}]});
            format!("data: {v}\n\n").into()
        })
        .chain([bytes::Bytes::from_static(b"data: [DONE]\n\n")])
        .collect();
    let headers = vec![("content-type".into(), "text/event-stream".into())];
    k.s.mock.on("/p", [Step::Stream { status: 200, headers, frames, every: Duration::from_millis(150), cut: false }]);
    let req = request(&k.s, "openai-chat", "p/m1", chat_body("p/m1", true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Ok(Answer::Events { mut rx, .. }) = k.engine().text(k.engine().snapshot(), req).await else {
        panic!("a stream")
    };
    let first = rx.recv().await.unwrap();
    assert_eq!(k.engine().refresh_account("p", "a").await, Refreshed::Fresh, "refreshed mid-stream");
    assert_ne!(k.access("p", "a"), old);
    let mut pieces = vec![first];
    pieces.extend(drain(&mut rx).await);
    let text: String = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Frame(f, _) => Some(f.data.clone()),
            _ => None,
        })
        .collect();
    for i in 0..6 {
        assert!(text.contains(&format!("w{i} ")), "the stream ran to its end: {text}");
    }
    assert!(!pieces.iter().any(|p| matches!(p, Piece::Restart | Piece::Event(Event::Error(_)))));
    let r = settled(&k.s, &id).await;
    assert_eq!(r.attempts.len(), 1, "never interrupted or retried");
    let got = k.s.mock.received();
    assert_eq!(got.len(), 1);
    assert_eq!(bearer(&got[0]), old, "sent with its original token");
}

#[tokio::test]
async fn the_maintenance_queue_runs_at_most_four_jobs_at_once() {
    let names = ["a", "b", "c", "d", "e", "f"];
    let k = kit(&[("p", Shape::Device, "5m")], &names, HOUR).await;
    for n in names {
        // Due now (within the 30 min lead), still valid.
        k.sign_in("p", n, |e| {
            e.expires_at = SystemTime::now() + Duration::from_secs(600);
            e.signed_in_at = SystemTime::now() - HOUR;
        });
    }
    k.idp.fail_token(names.map(|_| nullrouter_engine::testkit::Failure::Hang(Duration::from_millis(600))));
    let stop = upkeep(&k);
    eventually(Duration::from_secs(1), || k.idp.token_calls() >= maintenance::MAX_JOBS).await;
    assert_eq!(k.idp.token_calls(), maintenance::MAX_JOBS);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(k.idp.token_calls(), maintenance::MAX_JOBS, "the others wait for a free slot");
    assert!(eventually(Duration::from_secs(2), || k.idp.token_calls() == names.len()).await);
    let failed_once = || names.iter().all(|n| k.engine().refresher.attempts("p", n) == 1);
    assert!(eventually(Duration::from_secs(2), failed_once).await, "a hung refresh (504) is transient");
    stop.cancel();
    for n in names {
        assert_eq!(k.engine().tokens.get("p", n).unwrap().state, nullrouter_engine::tokens::AccountState::Active);
    }
}

#[tokio::test]
async fn a_paused_proxy_stops_a_token_refresh_before_anything_is_sent() {
    use nullrouter_engine::accounts::{self, Accounts};
    use nullrouter_engine::connection::fingerprints;
    use nullrouter_engine::testkit::MockProxy;

    let k = kit(&[("p", Shape::Device, "5m")], &["a"], HOUR).await;
    k.sign_in("p", "a", |_| {});
    let proxy = MockProxy::start().await;
    nullrouter_engine::files::write_private(
        &k.home().join("proxies.toml"),
        &format!("schema = 1\n\n[[proxy]]\nname = \"eu\"\nurl = \"http://{}\"\n", proxy.addr()),
    )
    .unwrap();
    let mut list = Accounts::load(&k.home().join(accounts::FILE)).unwrap();
    list.set_proxy("p", "a", Some("eu".into())).unwrap();
    list.save().unwrap();
    k.engine().reload().await.unwrap();

    // Through a healthy proxy the refresh goes out, and the proxy carried it.
    assert_eq!(k.engine().refresh_account("p", "a").await, Refreshed::Fresh);
    assert_eq!(k.idp.refresh_calls(), 1);
    assert!(proxy.carried() >= 1, "the refresh used the account's proxy");

    let print = fingerprints(&k.engine().snapshot()).remove("eu").unwrap();
    k.engine().proxy_board.pause("eu", "connect to proxy failed", &print);
    let (carried, calls) = (proxy.carried(), k.idp.refresh_calls());
    assert_eq!(k.engine().refresh_account("p", "a").await, Refreshed::Transient("proxy eu paused".into()));
    assert_eq!((proxy.carried(), k.idp.refresh_calls()), (carried, calls), "nothing was sent");
}
