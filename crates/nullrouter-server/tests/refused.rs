//! Refused by the provider (spec 005 T058; FR-004b, research R10): a fresh token rejected
//! again right after its refresh, and a response matching the plugin's
//! `[[signin.refused]]` rules, both mark the account `refused` with the provider's reason,
//! and the request falls back. A 403 that reads as model access stays an ordinary failure.
//! `accounts enable` (clearing the state, then a reload) puts the account back.

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{IdpServer, bearer, idp_server, reply_by_wire, style_request};
use nullrouter_engine::accounts;
use nullrouter_engine::records::{AttemptOutcome, ErrorClass, Outcome, RequestRecord};
use nullrouter_engine::testkit::Step;
use nullrouter_engine::tokens::{self, PersistedState, TokenStore};
use nullrouter_server::operator;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const RULE: &str =
    "[[signin.refused]]\nstatus = [400, 403]\nbody_contains = \"only authorized for use with Claude Code\"\n";
const REFUSAL: &str =
    "This credential is only authorized for use with Claude Code and cannot be used for other API requests.";

async fn state_of(s: &IdpServer, name: &str) -> Value {
    let v = operator::handle(&s.s.engine, &json!({"op": "accounts.state"})).await;
    v["accounts"].as_array().unwrap().iter().find(|a| a["name"] == name).unwrap().clone()
}

/// Sends one chat request; returns its status and settled record.
async fn send(s: &IdpServer) -> (u16, RequestRecord) {
    let r = style_request(&s.s.base, &s.s.key, "openai-chat", "p/m1").send().await.unwrap();
    let status = r.status().as_u16();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    for _ in 0..400 {
        let rec = s.s.engine.records.get(&id).unwrap();
        if rec.outcome != Outcome::InProgress {
            return (status, rec);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("record {id} never settled");
}

/// The upstream: `b`'s token answers; every token of `a` gets `a_gets`.
fn split(s: &IdpServer, a_gets: impl Fn() -> Step + Send + Sync + 'static) {
    let b: HashSet<String> =
        [s.s.engine.tokens.get("p", "b").unwrap().entry.access_token.with_exposed(str::to_owned)].into();
    s.s.mock.respond(move |r| if b.contains(&bearer(r)) { reply_by_wire(r) } else { a_gets() });
}

fn classes(rec: &RequestRecord) -> Vec<(Option<String>, Option<ErrorClass>)> {
    rec.attempts
        .iter()
        .map(|a| {
            let class = match &a.outcome {
                Some(AttemptOutcome::Failed { class, .. }) => Some(*class),
                Some(AttemptOutcome::Skipped { class, .. }) => *class,
                _ => None,
            };
            (a.account.clone(), class)
        })
        .collect()
}

#[tokio::test]
async fn a_fresh_token_rejected_again_is_refused_and_enable_clears_it() {
    let s = idp_server(&["a", "b"], "").await;
    split(&s, || Step::json(401, json!({"error": {"message": "invalid credentials for this client"}})));

    let (status, rec) = send(&s).await;
    assert_eq!(status, 200, "falls back to b: {rec:#?}");
    assert_eq!(s.idp.refresh_calls(), 1, "a's token was refreshed once");
    let c = classes(&rec);
    assert_eq!(c.first(), Some(&(Some("a".into()), Some(ErrorClass::Auth))), "the first rejection: {c:?}");
    assert!(c.contains(&(Some("a".into()), Some(ErrorClass::Refused))), "the second marks it: {c:?}");
    assert_eq!(c.last(), Some(&(Some("b".into()), None)));

    let a = state_of(&s, "a").await;
    assert_eq!(a["state"], "refused", "{a}");
    assert_eq!(a["state_reason"], "invalid credentials for this client");
    assert!(a["state_since"].is_string());
    let stored = TokenStore::load(s.s.home()).unwrap();
    assert_eq!(stored.get("p", "a").unwrap().state, Some(PersistedState::Refused), "kept across restarts");

    // Later requests don't call `a`; a plan that reaches it skips it with its class and
    // the command.
    let before = s.s.mock.received().len();
    let (status, _) = send(&s).await;
    assert_eq!(status, 200);
    assert_eq!(s.s.mock.received().len(), before + 1, "only b was called");
    let st = s.s.engine.snapshot();
    let w = accounts::out_of_service(st.accounts.get("p", "a").unwrap(), &st.tokens).unwrap();
    assert_eq!(w.class(), Some(ErrorClass::Refused));
    assert_eq!(
        w.to_string(),
        "refused by provider: invalid credentials for this client; run nullrouter accounts signin p a"
    );

    // `accounts enable` clears the state; after the reload `a` is tried again.
    assert!(tokens::clear_refused(s.s.home(), "p", "a").unwrap());
    assert_eq!(operator::handle(&s.s.engine, &json!({"op": "reload"})).await["ok"], true);
    assert_eq!(state_of(&s, "a").await["state"], "active");
    let st = s.s.engine.snapshot();
    assert_eq!(accounts::out_of_service(st.accounts.get("p", "a").unwrap(), &st.tokens), None);
    // `b` (the warm account) fails now: `a` answers.
    let b = s.s.engine.tokens.get("p", "b").unwrap().entry.access_token.with_exposed(str::to_owned);
    s.s.mock.respond(move |r| {
        if bearer(r) == b { Step::json(404, json!({"error": {"message": "no such model"}})) } else { reply_by_wire(r) }
    });
    let (status, rec) = send(&s).await;
    assert_eq!(status, 200, "{rec:#?}");
    let last = rec.attempts.last().unwrap();
    assert_eq!((last.account.as_deref(), &last.outcome), (Some("a"), &Some(AttemptOutcome::Ok)));
}

#[tokio::test]
async fn a_response_matching_the_refused_rule_marks_the_account_at_once() {
    let s = idp_server(&["a", "b"], RULE).await;
    split(&s, || {
        Step::json(400, json!({"type": "error", "error": {"type": "invalid_request_error", "message": REFUSAL}}))
    });

    let (status, rec) = send(&s).await;
    assert_eq!(status, 200, "{rec:#?}");
    assert_eq!(s.idp.refresh_calls(), 0, "a refusal isn't an expired token");
    assert_eq!(classes(&rec), [(Some("a".into()), Some(ErrorClass::Refused)), (Some("b".into()), None)]);
    let a = state_of(&s, "a").await;
    assert_eq!((a["state"].as_str(), a["state_reason"].as_str()), (Some("refused"), Some(REFUSAL)), "{a}");
}

#[tokio::test]
async fn a_model_access_403_stays_an_ordinary_failure() {
    let model_access = || Step::json(403, json!({"error": {"message": "You do not have access to model m1"}}));
    // Straight away, and right after a refresh that a 401 started.
    for after_refresh in [false, true] {
        let s = idp_server(&["a", "b"], RULE).await;
        let first = Arc::new(Mutex::new(after_refresh));
        split(&s, move || {
            if std::mem::take(&mut *first.lock().unwrap()) {
                Step::json(401, json!({"error": {"message": "token expired"}}))
            } else {
                model_access()
            }
        });
        let (status, rec) = send(&s).await;
        assert_eq!(status, 200, "{rec:#?}");
        assert_eq!(s.idp.refresh_calls(), usize::from(after_refresh));
        let c = classes(&rec);
        assert!(!c.iter().any(|(_, class)| *class == Some(ErrorClass::Refused)), "{c:?}");
        assert!(c.contains(&(Some("a".into()), Some(ErrorClass::Auth))), "{c:?}");
        assert_eq!(state_of(&s, "a").await["state"], "active", "after_refresh {after_refresh}");
        assert_eq!(TokenStore::load(s.s.home()).unwrap().get("p", "a").unwrap().state, None);
    }
}
