//! Spec 011 T029, US3, SC-003: retests on a simulated clock. The retester is asked what is due
//! at a time the test picks, and each retest's verdict is dated at that time, so six hours
//! pass in a moment.

mod common;
mod signin_kit;

use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::maintenance;
use nullrouter_engine::signin::refresh::Timing;
use nullrouter_engine::testkit::Step;
use nullrouter_engine::tests::retest::{self, Retester};
use nullrouter_engine::tests::{self, Planned, TestResult};
use nullrouter_engine::verdict::{Pair, Source, State, Verdict};
use serde_json::json;
use signin_kit::{Shape, eventually, kit};
use tokio_util::sync::CancellationToken;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 503 = { retries = 0 } }";
const SECOND: Duration = Duration::from_secs(1);

async fn alpha(accounts: &[(&str, &str)], config: &str) -> Setup {
    setup(|m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY))], accounts, config).await
}

fn pair(account: &str) -> Pair {
    Pair::new("alpha", account, "m1")
}

fn planned(s: &Setup, account: &str) -> Planned {
    let st = s.engine.snapshot();
    tests::expand(&s.engine, &st, Some("alpha/m1"), Some(account)).unwrap().remove(0)
}

/// Tests `p` as `source` with its verdict dated `at`.
async fn test_at(s: &Setup, p: &Planned, source: Source, at: SystemTime) -> TestResult {
    tests::run_pair_at(&s.engine, p, source, "tr_t", &CancellationToken::new(), Some(at)).await.unwrap()
}

fn unavailable() -> Step {
    Step::json(503, json!({"error": {"message": "service unavailable"}}))
}

fn not_found() -> Step {
    Step::json(404, json!({"error": {"message": "The model 'm1' does not exist"}}))
}

/// An UNKNOWN at `at`, step `step`, on the basis the pair has now.
fn unknown(s: &Setup, pair: &Pair, at: SystemTime, step: u32) -> Verdict {
    let st = s.engine.snapshot();
    Verdict {
        state: State::Unknown,
        reason: "503: service unavailable".into(),
        rejection: None,
        source: Source::Test,
        at,
        record: None,
        step: Some(step),
        next: None,
        basis: tests::basis(&s.engine, &st, pair),
        note: None,
    }
}

/// The pairs `r` starts at `now`.
fn due_at(s: &Setup, r: &mut Retester, now: SystemTime) -> Vec<Pair> {
    r.due(&s.engine, now).0.into_iter().map(|p| p.pair).collect()
}

/// Runs every retest due at the verdict's `next` until it has none, dating each at its due
/// time; returns the gaps between tests in seconds.
async fn retest_until_settled(s: &Setup, r: &mut Retester, account: &str, from: SystemTime) -> Vec<u64> {
    let (mut at, mut gaps) = (from, Vec::new());
    while let Some(next) = s.engine.verdicts.get(&pair(account)).unwrap().next {
        assert!(due_at(s, r, next - SECOND).is_empty(), "not before it is due");
        let (due, _) = r.due(&s.engine, next);
        assert_eq!(due.iter().map(|p| &p.pair).collect::<Vec<_>>(), [&pair(account)]);
        test_at(s, &due[0], Source::Retest, next).await;
        r.done(&due[0].pair);
        gaps.push(next.duration_since(at).unwrap().as_secs());
        at = next;
        assert!(gaps.len() <= 8, "{gaps:?}");
    }
    gaps
}

#[tokio::test]
async fn unknown_is_retested_at_1_5_30_min_then_every_6_h_until_it_settles() {
    let s = alpha(&[("alpha", "main")], "").await;
    s.mock.on("/alpha", [unavailable(), unavailable(), unavailable(), unavailable(), unavailable(), ok()]);
    let t0 = SystemTime::now();
    assert_eq!(test_at(&s, &planned(&s, "main"), Source::Test, t0).await.state, Some(State::Unknown));

    let mut r = Retester::default();
    let gaps = retest_until_settled(&s, &mut r, "main", t0).await;
    assert_eq!(gaps, [60, 300, 1800, 6 * 3600, 6 * 3600], "SC-003");
    let v = s.engine.verdicts.get(&pair("main")).unwrap();
    assert_eq!((v.state, v.source, v.next), (State::Pass, Source::Retest, None));
    // Settled: nothing more, however long it waits.
    let later = v.at + Duration::from_secs(30 * 24 * 3600);
    assert!(due_at(&s, &mut r, later).is_empty());
    assert_eq!(s.mock.received().len(), 6);
}

#[tokio::test]
async fn a_new_unknown_from_a_test_restarts_at_the_first_step() {
    let s = alpha(&[("alpha", "main")], "").await;
    s.mock.on("/alpha", [unavailable(), unavailable(), unavailable(), unavailable()]);
    let p = planned(&s, "main");
    let t0 = SystemTime::now();
    test_at(&s, &p, Source::Test, t0).await;
    test_at(&s, &p, Source::Retest, t0 + 60 * SECOND).await;
    test_at(&s, &p, Source::Retest, t0 + 360 * SECOND).await;
    assert_eq!(s.engine.verdicts.get(&pair("main")).unwrap().step, Some(2));

    let t1 = t0 + 400 * SECOND;
    test_at(&s, &p, Source::Test, t1).await;
    let v = s.engine.verdicts.get(&pair("main")).unwrap();
    assert_eq!((v.step, v.next), (Some(0), Some(t1 + 60 * SECOND)), "FR-012");
}

#[tokio::test]
async fn broken_is_retested_only_when_turned_on_and_never_when_the_operator_set_it() {
    let s = alpha(&[("alpha", "main")], "").await;
    s.mock.on("/alpha", [not_found()]);
    let t0 = SystemTime::now();
    assert_eq!(test_at(&s, &planned(&s, "main"), Source::Test, t0).await.state, Some(State::Broken));
    let mut r = Retester::default();
    assert_eq!(s.engine.verdicts.get(&pair("main")).unwrap().next, None);
    assert!(due_at(&s, &mut r, t0 + Duration::from_secs(400 * 24 * 3600)).is_empty(), "off by default");

    let s = alpha(&[("alpha", "main"), ("alpha", "ops")], "[tests]\nbroken_retest = \"on\"\n").await;
    s.mock.on("/alpha", [not_found(), not_found()]);
    let day = Duration::from_secs(24 * 3600);
    test_at(&s, &planned(&s, "main"), Source::Test, t0).await;
    assert_eq!(s.engine.verdicts.get(&pair("main")).unwrap().next, Some(t0 + day));
    let mut operator = unknown(&s, &pair("ops"), t0, 0);
    (operator.state, operator.source, operator.step) = (State::Broken, Source::Operator, None);
    s.engine.verdicts.set(pair("ops"), operator);

    let mut r = Retester::default();
    assert!(due_at(&s, &mut r, t0 + day - SECOND).is_empty());
    assert_eq!(due_at(&s, &mut r, t0 + day), [pair("main")], "the operator's BROKEN is never retested");
    let v = s.engine.verdicts.get(&pair("main")).unwrap();
    assert_eq!(retest::due(&v, &s.engine.snapshot().registry.runtime().tests), Some(t0 + day));
}

#[tokio::test]
async fn a_retest_waits_while_the_account_is_disabled_then_runs() {
    let accounts = |disabled: bool| {
        format!(
            "schema = 1\n[[account]]\nprovider = \"alpha\"\nname = \"main\"\nsecret = \"{SECRET}-alpha-main\"\ndisabled = {disabled}\n"
        )
    };
    let s = setup_file(|m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY))], &accounts(true), "").await;
    let t0 = SystemTime::now();
    s.engine.verdicts.set(pair("main"), unknown(&s, &pair("main"), t0, 0));
    let mut r = Retester::default();
    let due = t0 + 60 * SECOND;
    assert!(due_at(&s, &mut r, due).is_empty());
    let st = s.engine.snapshot();
    assert_eq!(retest::waiting(&s.engine, &st, &pair("main"), due).as_deref(), Some("account disabled"));
    assert_eq!(s.engine.verdicts.get(&pair("main")).unwrap().state, State::Unknown, "the verdict is kept");

    nullrouter_engine::files::write_private(&s._dir.path().join(nullrouter_engine::accounts::FILE), &accounts(false))
        .unwrap();
    s.engine.reload().await.unwrap();
    assert_eq!(due_at(&s, &mut r, due + SECOND), [pair("main")], "runs once it can serve");
}

#[tokio::test]
async fn a_retest_waits_out_a_rate_limit() {
    let s = alpha(&[("alpha", "main")], "").await;
    let limited = Step::Reply {
        status: 429,
        headers: vec![("retry-after".into(), "600".into()), ("content-type".into(), "application/json".into())],
        body: json!({"error": {"message": "slow down"}}).to_string().into(),
    };
    s.mock.on("/alpha", [limited]);
    let t0 = SystemTime::now();
    assert_eq!(test_at(&s, &planned(&s, "main"), Source::Test, t0).await.state, Some(State::Unknown));
    let mut r = Retester::default();
    let due = t0 + 60 * SECOND;
    assert!(due_at(&s, &mut r, due).is_empty());
    let st = s.engine.snapshot();
    let why = retest::waiting(&s.engine, &st, &pair("main"), due).unwrap();
    assert!(why.starts_with("rate-limited until "), "{why}");
}

#[tokio::test]
async fn a_changed_schedule_applies_to_the_next_retest_without_a_restart() {
    let s = alpha(&[("alpha", "main")], "").await;
    let t0 = SystemTime::now();
    s.engine.verdicts.set(pair("main"), unknown(&s, &pair("main"), t0, 0));
    let mut r = Retester::default();
    assert_eq!(due_at(&s, &mut r, t0 + 60 * SECOND), [pair("main")]);
    r.done(&pair("main"));

    std::fs::write(
        s._dir.path().join("config.toml"),
        "allow_private_endpoints = true\n[tests]\nretest = [\"2m\", \"10m\"]\n",
    )
    .unwrap();
    s.engine.reload().await.unwrap();
    assert!(due_at(&s, &mut r, t0 + 119 * SECOND).is_empty());
    assert_eq!(due_at(&s, &mut r, t0 + 120 * SECOND), [pair("main")]);
}

#[tokio::test]
async fn overdue_retests_after_a_restart_start_10_s_apart() {
    let s = alpha(&[("alpha", "a"), ("alpha", "b"), ("alpha", "c")], "").await;
    let now = SystemTime::now();
    let hour_ago = now - Duration::from_secs(3600);
    for a in ["a", "b", "c"] {
        s.engine.verdicts.set(pair(a), unknown(&s, &pair(a), hour_ago, 1));
    }
    let mut r = Retester::default();
    let (first, next) = r.due(&s.engine, now);
    assert_eq!(first.len(), 1);
    assert_eq!(next, now + retest::SPREAD);
    assert!(due_at(&s, &mut r, now + 9 * SECOND).is_empty());
    assert_eq!(due_at(&s, &mut r, now + 10 * SECOND).len(), 1);
    assert_eq!(due_at(&s, &mut r, now + 20 * SECOND).len(), 1);
    assert_eq!(r.running(), 3);
}

#[tokio::test]
async fn two_tests_of_one_pair_keep_the_one_that_finished_last() {
    let s = alpha(&[("alpha", "main")], "[tests.timeout]\ntext = \"5s\"\n").await;
    s.mock.on("/alpha", [Step::StallHeaders { hold: Duration::from_secs(60) }, ok()]);
    let p = planned(&s, "main");
    let slow = tests::run_pair(&s.engine, &p, Source::Test, "tr_1", &CancellationToken::new());
    let fast = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        tests::run_pair(&s.engine, &p, Source::Retest, "tr_2", &CancellationToken::new()).await
    };
    let (slow, fast) = tokio::join!(slow, fast);
    assert_eq!(fast.unwrap().state, Some(State::Pass));
    assert_eq!(slow.unwrap().state, Some(State::Unknown));
    let v = s.engine.verdicts.get(&pair("main")).unwrap();
    assert_eq!((v.state, v.source), (State::Unknown, Source::Test), "the later finish is kept");
}

/// Retests filling the test limit hold no maintenance slot: a token refresh due meanwhile
/// still runs on time (research R8).
#[tokio::test]
async fn a_token_refresh_runs_on_time_while_retests_fill_the_test_limit() {
    let k = kit(&[("p", Shape::Device, "1s")], &["a"], Duration::from_secs(6)).await;
    k.engine().refresher.set_timing(Timing {
        min_lead: Duration::from_secs(2),
        use_margin: Duration::from_millis(100),
        ..Timing::default()
    });
    k.sign_in("p", "a", |_| {});
    let refresh = k.stored("p", "a").refresh_token.unwrap().with_exposed(str::to_owned);
    let engine = k.engine().clone();
    let limit = engine.snapshot().registry.runtime().tests.concurrency;
    let mut held = Vec::new();
    for _ in 0..limit {
        held.push(engine.test_gate.enter(limit).await);
    }
    // Overdue retests, each waiting for the gate.
    let st = engine.snapshot();
    for model in ["m1", "m2", "m3", "m4", "m5"] {
        let pair = Pair::new("p", "a", model);
        let v = Verdict {
            state: State::Unknown,
            reason: "503".into(),
            rejection: None,
            source: Source::Test,
            at: SystemTime::now() - Duration::from_secs(3600),
            record: None,
            step: Some(0),
            next: None,
            basis: tests::basis(&engine, &st, &pair),
            note: None,
        };
        engine.verdicts.set(pair, v);
    }
    let stop = CancellationToken::new();
    let (a, b) = (stop.clone(), stop.clone());
    let retests = retest::spawn(engine.clone(), async move { a.cancelled().await });
    let upkeep = maintenance::spawn(engine.clone(), async move { b.cancelled().await });

    let refreshed = eventually(Duration::from_secs(8), || {
        k.idp.token_requests().iter().any(|t| t.get("refresh_token") == Some(refresh.as_str()))
    })
    .await;
    stop.cancel();
    let _ = retests.await;
    let _ = upkeep.await;
    assert!(refreshed, "the refresh ran while the test limit was full");
    assert!(k.s.mock.received().is_empty(), "no retest got past the full limit");
    drop(held);
}
