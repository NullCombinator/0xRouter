//! Same-account retry budgets (T063, research R7).
//!
//! The default table is checked through `classify::budget`, and the loop's use of it with
//! plugin overrides at short delays. Paused tokio time isn't used: it auto-advances while
//! the engine waits on the loopback socket, which fires header timeouts that never
//! happened.

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::classify::{self, Budget};
use nullrouter_engine::records::{AttemptKind, ErrorClass};
use nullrouter_engine::testkit::Step;
use nullrouter_registry::schema::RetryOverride;
use serde_json::json;

fn b(retries: u32, secs: u64) -> Budget {
    Budget { retries, delay: Duration::from_secs(secs) }
}

fn default(status: Option<u16>, verdict: classify::Verdict, indicated: Option<Duration>) -> Budget {
    classify::budget(status, &verdict, indicated, &BTreeMap::new())
}

#[test]
fn the_default_budgets_follow_r7() {
    let up = |s: u16| (Some(s), classify::upstream(s, "boom"));
    for (status, verdict) in
        [up(502), (None, classify::transport(ErrorClass::Network)), (None, classify::transport(ErrorClass::Timeout))]
    {
        assert_eq!(default(status, verdict, None), b(3, 3), "{status:?} {verdict:?}");
    }
    let (s, v) = up(503);
    assert_eq!(default(s, v, None), b(3, 2));
    let (s, v) = up(504);
    assert_eq!(default(s, v, None), b(2, 3));
    for code in [500, 501, 505, 529, 599] {
        let (s, v) = up(code);
        assert_eq!(default(s, v, None), b(1, 2), "{code}");
    }
    for code in [401, 402, 403, 404] {
        let (s, v) = up(code);
        assert_eq!(default(s, v, None), b(0, 0), "{code}");
    }
    let (s, v) = up(400);
    assert_eq!(default(s, v, None), b(0, 0), "no fallback, no retry");
}

#[test]
fn a_rate_limit_waits_only_when_the_provider_says_briefly() {
    let v = classify::upstream(429, "slow down");
    assert_eq!(
        default(Some(429), v, Some(Duration::from_millis(1500))),
        Budget { retries: 1, delay: Duration::from_millis(1500) }
    );
    assert_eq!(default(Some(429), v, Some(Duration::from_secs(5))), b(1, 5));
    assert_eq!(default(Some(429), v, Some(Duration::from_secs(6))), b(0, 0), "a longer wait moves on at once");
    assert_eq!(default(Some(429), v, None), b(1, 2));
}

#[test]
fn a_plugin_override_wins_for_its_status() {
    let mut o = BTreeMap::new();
    o.insert("503".to_owned(), RetryOverride { retries: 5, delay_ms: 10 });
    let v = classify::upstream(503, "boom");
    assert_eq!(classify::budget(Some(503), &v, None, &o), Budget { retries: 5, delay: Duration::from_millis(10) });
    let v = classify::upstream(502, "boom");
    assert_eq!(classify::budget(Some(502), &v, None, &o), b(3, 3), "other statuses keep the table");
}

#[test]
fn the_reset_headers_give_the_indicated_wait() {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("retry-after", "2".parse().unwrap());
    assert_eq!(classify::indicated_wait(&h, SystemTime::now()), Some(Duration::from_secs(2)));
}

async fn one_account(endpoint: &str) -> Setup {
    let endpoint = endpoint.to_owned();
    setup(move |m| vec![("alpha", chat_plugin(m, "alpha", &endpoint))], &[("alpha", "main")], "").await
}

#[tokio::test]
async fn the_loop_retries_the_same_account_by_the_override_then_answers() {
    let s = one_account("retry = { 503 = { retries = 3, delay_ms = 50 } }").await;
    s.mock.push([err(503), err(503), err(503), ok()]);
    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    let got = s.mock.received();
    assert_eq!(got.len(), 4);
    for w in got.windows(2) {
        assert!(w[1].at - w[0].at >= Duration::from_millis(45), "the delay is kept: {:?}", w[1].at - w[0].at);
    }
    let r = s.engine.records.get(&id).unwrap();
    let kinds: Vec<_> = r.attempts.iter().map(|a| a.kind).collect();
    assert_eq!(
        kinds,
        [
            AttemptKind::Initial,
            AttemptKind::SameAccountRetry,
            AttemptKind::SameAccountRetry,
            AttemptKind::SameAccountRetry
        ]
    );
}

#[tokio::test]
async fn a_spent_budget_moves_on_and_the_line_counts_the_retries() {
    let s = one_account("retry = { 502 = { retries = 2, delay_ms = 10 } }").await;
    s.mock.push([err(502), err(502), err(502), ok()]);
    let (_, res) = send(&s, "alpha/m1").await;
    let f = res.err().expect("one account, budget spent");
    assert_eq!(s.mock.received().len(), 3, "1 + 2 retries, then no more");
    assert_eq!(f.status, 503);
    assert_eq!(f.tried.len(), 1);
    assert_eq!(f.tried[0].retries, 2);
    assert!(f.message.contains("after 2 retries"), "{}", f.message);
}

#[tokio::test]
async fn auth_and_not_found_are_not_retried() {
    for code in [401, 402, 403, 404] {
        let s = one_account("").await;
        s.mock.push([err(code), ok()]);
        let (_, res) = send(&s, "alpha/m1").await;
        assert!(res.is_err(), "{code}");
        assert_eq!(s.mock.received().len(), 1, "{code}");
    }
}

#[tokio::test]
async fn a_short_retry_after_is_waited_out_on_the_same_account() {
    let s = one_account("").await;
    s.mock.push([Step::rate_limited(1, json!({"error": {"message": "slow down"}})), ok()]);
    let (_, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    let got = s.mock.received();
    assert_eq!(got.len(), 2);
    assert!(got[1].at - got[0].at >= Duration::from_millis(950), "{:?}", got[1].at - got[0].at);
}

#[tokio::test]
async fn a_long_retry_after_moves_on_at_once() {
    let s =
        setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "main"), ("alpha", "backup")], "").await;
    s.mock.push([Step::rate_limited(30, json!({"error": {"message": "slow down"}})), ok()]);
    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    let got = s.mock.received();
    assert!(got[1].at - got[0].at < Duration::from_millis(900), "no wait: {:?}", got[1].at - got[0].at);
    assert_eq!(accounts_hit(&s), ["alpha-main", "alpha-backup"]);
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(trail(&r), [t("alpha", "main", AttemptKind::Initial), t("alpha", "backup", AttemptKind::NextAccount)]);
}
