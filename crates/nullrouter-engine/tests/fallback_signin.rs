//! Fallback through sign-in accounts (spec 005 T027, SC-004, FR-007): two grok-cli sign-in
//! accounts plus a key-account member. A 429, a 5xx, a timeout or a connection failure on
//! the first account never reaches the client, and the attempt order is slice 003's: the
//! same scenario over key accounts gives the same trail.

mod common;

use common::*;
use nullrouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass, RequestRecord};
use nullrouter_engine::testkit::{MockUpstream, Step};
use std::time::Duration;

const NO_RETRY: &str = "retry = { 429 = { retries = 0 }, 500 = { retries = 0 }, 502 = { retries = 0 }, 503 = { retries = 0 }, 504 = { retries = 0 } }";

/// grok-cli's sections as provider `grokmock` (the bundled grok-cli stays), pointed at
/// `url`: Responses with `force_stream`, a device-code sign-in and its identity headers. `keyed` adds an `[auth]` so key accounts can stand in.
fn grok_cli(mock: &MockUpstream, url: &str, keyed: bool) -> String {
    let auth = if keyed { "[auth]\nkind = \"apikey\"\n" } else { "" };
    format!(
        r#"schema = 2
id = "grokmock"
category = "apikey"
{auth}[endpoints.text]
url = "{url}"
wire = "openai-responses"
force_stream = true
timeout_ms = 300
{NO_RETRY}
[signin]
flow = "device_code"
client_id = "test-client"
device_url = "{device}"
token_url = "{token}"
refresh_lead = "5m"
[identity.headers]
User-Agent = "grok-shell/0.2.99 (linux; x86_64)"
x-grok-req-id = "{{request.id}}"
x-email = "{{account.email}}"
[[models]]
id = "m1"
"#,
        device = mock.url("/idp/device"),
        token = mock.url("/idp/token"),
    )
}

/// `grokmock` accounts `a` and `b` (sign-in unless `keyed`) and `keyco/main`, under
/// unified model `u`. `url`: grokmock's endpoint (the mock unless a connection must fail).
async fn three(keyed: bool, url: Option<String>) -> Setup {
    let signin: &[(&str, &str)] = if keyed { &[] } else { &[("grokmock", "a"), ("grokmock", "b")] };
    setup_signin(
        |m| {
            let url = url.unwrap_or_else(|| m.url("/grokmock/v1/responses"));
            vec![("grokmock", grok_cli(m, &url, keyed)), ("keyco", chat_plugin(m, "keyco", NO_RETRY))]
        },
        &[("grokmock", "a"), ("grokmock", "b"), ("keyco", "main")],
        signin,
        &unified(&[("grokmock", "m1"), ("keyco", "m1")]),
    )
    .await
}

fn class(r: &RequestRecord, n: usize) -> Option<ErrorClass> {
    match &r.attempts[n].outcome {
        Some(AttemptOutcome::Failed { class, .. }) => Some(*class),
        other => panic!("attempt {n}: {other:?}"),
    }
}

/// A port nothing listens on.
fn closed_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}/grokmock/v1/responses")
}

#[tokio::test]
async fn a_failing_first_sign_in_account_falls_back_like_a_key_account() {
    type Failure = (&'static str, fn() -> Step, ErrorClass);
    let failures: [Failure; 3] = [
        ("429", || err(429), ErrorClass::RateLimited),
        ("503", || err(503), ErrorClass::Transient),
        ("timeout", || Step::StallHeaders { hold: Duration::from_secs(5) }, ErrorClass::Timeout),
    ];
    for (what, fail, want) in failures {
        let mut trails = Vec::new();
        for keyed in [false, true] {
            let s = three(keyed, None).await;
            s.mock.on("/grokmock", [fail(), responses_stream()]);
            let (id, res) = send(&s, "u").await;
            assert!(res.is_ok(), "{what}: the failure never reaches the client");
            let r = settled(&s, &id).await;
            assert_eq!(class(&r, 0), Some(want), "{what}");
            assert_eq!(accounts_hit(&s), ["grokmock-a", "grokmock-b"], "{what}");
            assert_eq!(r.served_by.as_ref().unwrap().account.as_deref(), Some("b"), "{what}");
            trails.push(trail(&r));
        }
        assert_eq!(
            trails[0],
            [t("grokmock", "a", AttemptKind::Initial), t("grokmock", "b", AttemptKind::NextAccount)],
            "{what}"
        );
        assert_eq!(trails[0], trails[1], "{what}: sign-in accounts walk slice 003's order");
    }
}

#[tokio::test]
async fn both_sign_in_accounts_failing_hand_over_to_the_key_member() {
    let mut trails = Vec::new();
    for keyed in [false, true] {
        let s = three(keyed, None).await;
        s.mock.on("/grokmock", [err(500), err(502)]);
        s.mock.on("/keyco", [ok()]);
        let (id, res) = send(&s, "u").await;
        assert!(res.is_ok(), "{:?}", res.err());
        assert_eq!(accounts_hit(&s), ["grokmock-a", "grokmock-b", "keyco-main"]);
        trails.push(trail(&settled(&s, &id).await));
    }
    assert_eq!(
        trails[0],
        [
            t("grokmock", "a", AttemptKind::Initial),
            t("grokmock", "b", AttemptKind::NextAccount),
            t("keyco", "main", AttemptKind::NextMember)
        ]
    );
    assert_eq!(trails[0], trails[1]);
}

#[tokio::test]
async fn a_connection_failure_never_reaches_the_client() {
    let mut trails = Vec::new();
    for keyed in [false, true] {
        let s = three(keyed, Some(closed_url())).await;
        s.mock.on("/keyco", [ok()]);
        let (id, res) = send(&s, "u").await;
        assert!(res.is_ok(), "the key member serves");
        let r = settled(&s, &id).await;
        assert_eq!(class(&r, 0), Some(ErrorClass::Network));
        assert_eq!(paths(&s), ["/keyco/chat/completions"]);
        trails.push(trail(&r));
    }
    assert_eq!(
        trails[0],
        [
            t("grokmock", "a", AttemptKind::Initial),
            t("grokmock", "b", AttemptKind::NextAccount),
            t("keyco", "main", AttemptKind::NextMember)
        ]
    );
    assert_eq!(trails[0], trails[1]);
}

#[tokio::test]
async fn sign_in_requests_carry_the_token_and_identity_but_key_requests_dont() {
    let s = three(false, None).await;
    s.mock.on("/grokmock", [err(503)]);
    s.mock.on("/keyco", [ok()]);
    // Only one step is queued for /grokmock: account b gets a 404 and falls back.
    let (_, res) = send(&s, "u").await;
    assert!(res.is_ok());
    let got = s.mock.received();
    let first = &got[0];
    assert_eq!(first.headers["authorization"], format!("Bearer {SECRET}-grokmock-a"));
    assert_eq!(first.headers["user-agent"], "grok-shell/0.2.99 (linux; x86_64)");
    assert_eq!(first.headers["x-email"], "a@example.com");
    assert_eq!(got[1].headers["x-email"], "b@example.com");
    assert_ne!(first.headers["x-grok-req-id"], got[1].headers["x-grok-req-id"], "a fresh id per request");
    let key = got.last().unwrap();
    assert!(key.headers.get("x-email").is_none() && key.headers.get("x-grok-req-id").is_none());
}
