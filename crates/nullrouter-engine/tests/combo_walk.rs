//! Spec 011 T033, US5 scenarios 1–6, SC-006: a combo walks its unified models in order, moving
//! on only once a member has used up its own retries and fallbacks, never after output, and
//! never after a request error. Records name the combo and each attempt's member path.

mod common;

use std::time::{Duration, SystemTime};

use common::*;
use nullrouter_engine::attempt::Answer;
use nullrouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass, Outcome, RequestRecord};
use nullrouter_engine::testkit::Step;
use nullrouter_engine::verdict::{Basis, Pair, Rejection, Source, State, Verdict};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 500 = { retries = 0 }, 502 = { retries = 0 }, 503 = { retries = 0 }, 504 = { retries = 0 } }";

/// Providers `alpha` (accounts a, b) and `beta` (account a); unified models `ua` over alpha
/// and `ub` over beta; combos from `combos`.
async fn fleet(combos: &str) -> Setup {
    let config = format!(
        "[[unified_model]]\nname = \"ua\"\nmembers = [{{ provider = \"alpha\", model = \"m1\" }}]\n\
         [[unified_model]]\nname = \"ub\"\nmembers = [{{ provider = \"beta\", model = \"m1\" }}]\n{combos}"
    );
    setup(
        |m| vec![("alpha", chat_plugin(m, "alpha", NO_RETRY)), ("beta", chat_plugin(m, "beta", NO_RETRY))],
        &[("alpha", "a"), ("alpha", "b"), ("beta", "a")],
        &config,
    )
    .await
}

const CODER: &str = "[[combo]]\nname = \"coder\"\nmembers = [\"ua\", \"ub\"]\n";

/// `(member, provider, kind)` of every attempt.
fn walk(r: &RequestRecord) -> Vec<(Option<String>, String, AttemptKind)> {
    r.attempts.iter().map(|a| (a.member.clone(), a.provider.clone(), a.kind)).collect()
}

fn at(member: &str, provider: &str, kind: AttemptKind) -> (Option<String>, String, AttemptKind) {
    (Some(member.to_owned()), provider.to_owned(), kind)
}

/// Both alpha accounts, in the placement's order, then beta's.
fn alpha_twice_then_beta(s: &Setup) {
    let hits = accounts_hit(s);
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(hits[..2].contains(&"alpha-a".to_owned()) && hits[..2].contains(&"alpha-b".to_owned()), "{hits:?}");
    assert_eq!(hits[2], "beta-a");
}

#[tokio::test]
async fn the_first_member_answers() {
    let s = fleet(CODER).await;
    s.mock.on("/alpha", [ok()]);
    let (id, res) = send(&s, "coder").await;
    assert!(res.is_ok(), "{:?}", res.err());
    assert_eq!(paths(&s), ["/alpha/chat/completions"]);
    let r = settled(&s, &id).await;
    assert_eq!(r.combo.as_deref(), Some("coder"));
    assert_eq!(walk(&r), [at("coder › ua", "alpha", AttemptKind::Initial)]);
    assert_eq!(r.served_by.map(|b| b.provider), Some("alpha".into()));
}

#[tokio::test]
async fn the_next_member_only_after_every_account_of_the_first() {
    let s = fleet(CODER).await;
    s.mock.respond(|r| if r.path_and_query.starts_with("/alpha") { err(503) } else { ok() });
    let (id, res) = send(&s, "coder").await;
    assert!(res.is_ok(), "{:?}", res.err());
    alpha_twice_then_beta(&s);
    let r = settled(&s, &id).await;
    assert_eq!(
        walk(&r),
        [
            at("coder › ua", "alpha", AttemptKind::Initial),
            at("coder › ua", "alpha", AttemptKind::NextAccount),
            at("coder › ub", "beta", AttemptKind::NextMember),
        ]
    );
    assert_eq!(r.outcome, Outcome::Succeeded);
}

#[tokio::test]
async fn a_request_error_ends_the_combo() {
    let s = fleet(CODER).await;
    let bad = json!({"error": {"message": "max_tokens is too large", "type": "invalid_request_error"}});
    let bad = Step::json(400, bad);
    s.mock.on("/alpha", [bad]);
    let (id, res) = send(&s, "coder").await;
    let f = res.err().expect("no fallback on 400");
    assert_eq!(f.status, 400);
    assert_eq!(paths(&s), ["/alpha/chat/completions"], "FR-026: the next member is never tried");
    assert_eq!(settled(&s, &id).await.outcome, Outcome::Failed);
}

#[tokio::test]
async fn a_member_broken_on_every_account_is_skipped_without_a_call() {
    let s = fleet(CODER).await;
    let broken = Verdict {
        state: State::Broken,
        reason: "404: model m1 does not exist".into(),
        rejection: Some(Rejection::ModelNotFound),
        source: Source::Test,
        at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_400_000),
        record: None,
        step: None,
        next: None,
        basis: Basis::default(),
        note: None,
    };
    for a in ["a", "b"] {
        s.engine.verdicts.set(Pair::new("alpha", a, "m1"), broken.clone());
    }
    s.mock.on("/beta", [ok()]);
    let (id, res) = send(&s, "coder").await;
    assert!(res.is_ok(), "{:?}", res.err());
    assert_eq!(paths(&s), ["/beta/chat/completions"]);
    let r = settled(&s, &id).await;
    let skips: Vec<_> = r.attempts.iter().filter(|a| a.kind == AttemptKind::Skipped).collect();
    assert_eq!(skips.len(), 2, "{:?}", r.attempts);
    for skip in skips {
        assert_eq!(skip.member.as_deref(), Some("coder › ua"));
        let Some(AttemptOutcome::Skipped { class, .. }) = &skip.outcome else { panic!("{skip:?}") };
        assert_eq!(*class, Some(ErrorClass::Broken));
    }
}

#[tokio::test]
async fn a_unified_model_reached_twice_is_tried_once() {
    let s = fleet(
        "[[combo]]\nname = \"coder\"\nmembers = [\"ua\", \"chain\"]\n\
         [[combo]]\nname = \"chain\"\nmembers = [\"ua\", \"ub\"]\n",
    )
    .await;
    s.mock.respond(|r| if r.path_and_query.starts_with("/alpha") { err(503) } else { ok() });
    let (id, res) = send(&s, "coder").await;
    assert!(res.is_ok(), "{:?}", res.err());
    alpha_twice_then_beta(&s);
    let r = settled(&s, &id).await;
    let members: Vec<_> = r.attempts.iter().filter_map(|a| a.member.clone()).collect();
    assert_eq!(members, ["coder › ua", "coder › ua", "coder › chain › ub"]);
}

#[tokio::test]
async fn never_the_next_member_once_the_answer_has_started() {
    let s = fleet(CODER).await;
    let cut = Step::sse(
        &[(
            None,
            json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Hel"}, "finish_reason": Value::Null}]}),
        )],
        true,
    )
    .cut_after(1);
    s.mock.respond(move |r| if r.path_and_query.starts_with("/alpha") { cut.clone() } else { ok() });
    let req = request(&s, "openai-chat", "coder", chat_body("coder", true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Ok(Answer::Events { mut rx, .. }) = s.engine.text(s.engine.snapshot(), req).await else {
        panic!("expected a stream")
    };
    drain(&mut rx).await;
    assert!(
        paths(&s).iter().all(|p| p.starts_with("/alpha")),
        "FR-026: no member after output: {:?}",
        paths(&s)
    );
    let r = settled(&s, &id).await;
    assert!(r.attempts.iter().all(|a| a.member.as_deref() == Some("coder › ua")), "{:?}", r.attempts);
}
