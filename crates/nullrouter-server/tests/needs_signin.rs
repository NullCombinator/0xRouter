//! An account that can't be refreshed is named, and service carries on (spec 005 T057;
//! FR-015–FR-017, SC-005, research R10): a permanent refresh failure takes the account out,
//! the other account serves, and the operator sees the account and the command in the
//! operator socket, the records, and (with no other account) the informational error in
//! every style. The state survives a restart; signing in again brings the account back
//! under the same name without one.

mod common;

use std::time::{Duration, SystemTime};

use common::{IdpServer, STYLES, idp_server, style_request, write_grant};
use nullrouter_engine::records::{AttemptOutcome, ErrorClass, Outcome, RequestRecord};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::Failure;
use nullrouter_engine::tokens::{PersistedState, TokenStore};
use nullrouter_registry::OperatorHome;
use nullrouter_server::operator;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const COMMAND: &str = "run nullrouter accounts signin p a";

async fn state_of(engine: &std::sync::Arc<Engine>, name: &str) -> Value {
    let v = operator::handle(engine, &json!({"op": "accounts.state"})).await;
    v["accounts"].as_array().unwrap().iter().find(|a| a["name"] == name).unwrap().clone()
}

async fn settled(s: &IdpServer, id: &str) -> RequestRecord {
    for _ in 0..400 {
        let r = s.s.engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("record {id} never settled");
}

/// The skip of `p/a` in a record, as the records show it.
fn skipped_a(r: &RequestRecord) -> (String, Option<ErrorClass>) {
    let a = r.attempts.iter().find(|a| a.account.as_deref() == Some("a")).expect("an attempt of p/a");
    match &a.outcome {
        Some(AttemptOutcome::Skipped { reason, class }) => (reason.clone(), *class),
        other => panic!("p/a not skipped: {other:?}"),
    }
}

#[tokio::test]
async fn a_permanent_refresh_failure_is_named_and_service_carries_on() {
    let s = idp_server(&["a", "b"], "").await;
    let (engine, home) = (s.s.engine.clone(), s.s.home().to_owned());
    // `a`'s token has expired; its refresh is refused for good.
    write_grant(&home, &engine, &s.idp, "a", |e| {
        e.expires_at = SystemTime::now() - Duration::from_secs(1);
        e.signed_in_at = SystemTime::now() - Duration::from_secs(3600);
    });
    engine.reload_blocking().unwrap();
    s.idp.fail_token([Failure::permanent("invalid_grant")]);

    // Served by `b`; the record names `a` and the command.
    let r = style_request(&s.s.base, &s.s.key, "openai-chat", "p/m1").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let rec = settled(&s, &id).await;
    assert_eq!(rec.outcome, Outcome::Succeeded);
    assert_eq!(skipped_a(&rec), (format!("needs sign-in: {COMMAND}"), Some(ErrorClass::NeedsSignIn)));
    // `b` is now the warm account and serves later requests first.
    let r = style_request(&s.s.base, &s.s.key, "openai-chat", "p/m1").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(s.idp.refresh_calls(), 1, "a permanent failure isn't retried");

    // The operator socket: state, time and the provider's reason.
    let a = state_of(&engine, "a").await;
    assert_eq!((a["kind"].as_str(), a["state"].as_str()), (Some("signin"), Some("needs_sign_in")), "{a}");
    assert_eq!(a["state_reason"], "invalid_grant");
    assert!(a["state_since"].as_str().is_some_and(|t| t.ends_with('Z')), "{a}");
    assert!(a["expires_at"].is_string(), "{a}");
    assert_eq!(state_of(&engine, "b").await["state"], "active");
    let stored = TokenStore::load(&home).unwrap();
    assert_eq!(stored.get("p", "a").unwrap().state, Some(PersistedState::NeedsSignIn));

    // With no other account (`b` disabled), every style's error names `a` and the command,
    // in the message and in the structured details.
    let accounts = home.join(nullrouter_engine::accounts::FILE);
    let text = std::fs::read_to_string(&accounts).unwrap();
    let text = text.replace("name = \"b\"\n", "name = \"b\"\ndisabled = true\n");
    nullrouter_engine::files::write_private(&accounts, &text).unwrap();
    assert_eq!(operator::handle(&engine, &json!({"op": "reload"})).await["ok"], true);
    for style in STYLES {
        let r = style_request(&s.s.base, &s.s.key, style, "p/m1").send().await.unwrap();
        assert_eq!(r.status(), 503, "{style}");
        let body: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        let message = body["error"]["message"].as_str().unwrap_or_else(|| panic!("{style}: {body}"));
        assert!(message.contains(&format!("\np/a: needs sign-in — {COMMAND}")), "{style}: {message}");
        let details = if style == "anthropic-messages" { &body["nullrouter"] } else { &body["error"]["nullrouter"] };
        let at = &details["attempts"][0];
        assert_eq!((at["provider"].as_str(), at["account"].as_str()), (Some("p"), Some("a")), "{style}: {body}");
        assert_eq!(at["class"], "needs_sign_in", "{style}: {body}");
        assert!(at["reason"].as_str().unwrap().contains(COMMAND), "{style}: {body}");
        let id = details["record_id"].as_str().unwrap();
        assert_eq!(
            skipped_a(&settled(&s, id).await),
            (format!("needs sign-in: {COMMAND}"), Some(ErrorClass::NeedsSignIn))
        );
    }

    // A restart keeps the state.
    let (again, _) = Engine::open_parity(OperatorHome::new(&home)).unwrap();
    let again = std::sync::Arc::new(again);
    let a = state_of(&again, "a").await;
    assert_eq!((a["state"].as_str(), a["state_reason"].as_str()), (Some("needs_sign_in"), Some("invalid_grant")));
    drop(again);

    // Signing in again (as `accounts signin` does: write, then reload) brings `a` back,
    // same name, no restart.
    write_grant(&home, &engine, &s.idp, "a", |_| {});
    assert_eq!(operator::handle(&engine, &json!({"op": "reload"})).await["ok"], true);
    let a = state_of(&engine, "a").await;
    assert_eq!(a["state"], "active", "{a}");
    assert!(a["state_since"].is_null() && a["state_reason"].is_null(), "{a}");
    let r = style_request(&s.s.base, &s.s.key, "openai-chat", "p/m1").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let rec = settled(&s, &id).await;
    assert_eq!(rec.attempts.len(), 1, "{rec:#?}");
    assert_eq!(rec.attempts[0].account.as_deref(), Some("a"));
    assert_eq!(rec.attempts[0].outcome, Some(AttemptOutcome::Ok));
}
