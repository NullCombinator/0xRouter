//! Proxies per provider and account (spec 013, US4): the three levels and `none`, the pause
//! when a proxy is down (SC-011), `proxy fixed`, and a provider's own error through a healthy
//! proxy.
//!
//! The pause that has to leave no trace of a failed attempt is set directly on the engine's
//! board, so the cooldown a real connect failure would set doesn't hide what a skip does.

mod common;

use std::time::Duration;

use common::{SECRET, Server, chat_whole, server};
use nullrouter_engine::connection::fingerprints;
use nullrouter_engine::connection::pause::ProxyBoard;
use nullrouter_engine::records::{AttemptOutcome, RequestRecord};
use nullrouter_engine::testkit::{MockProxy, Step};
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const PASSWORD: &str = "pw-PROXY-SENTINEL-0007";

fn entry(name: &str, p: &MockProxy, credentials: bool) -> String {
    let auth = if credentials { format!("username = \"u\"\npassword = \"{PASSWORD}\"\n") } else { String::new() };
    format!("[[proxy]]\nname = \"{name}\"\nurl = \"http://{}\"\n{auth}\n", p.addr())
}

/// Writes `proxies.toml`, `accounts.toml` (the mock account naming `account_proxy`) and
/// `config.toml`, then reloads.
async fn apply(s: &Server, proxies: &[String], account_proxy: Option<&str>, config: &str) {
    nullrouter_engine::files::write_private(
        &s.home().join("proxies.toml"),
        &format!("schema = 1\n\n{}", proxies.concat()),
    )
    .unwrap();
    let proxy = account_proxy.map(|p| format!("proxy = \"{p}\"\n")).unwrap_or_default();
    nullrouter_engine::files::write_private(
        &s.home().join("accounts.toml"),
        &format!("schema = 2\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"{SECRET}\"\n{proxy}"),
    )
    .unwrap();
    std::fs::write(s.home().join("config.toml"), format!("allow_private_endpoints = true\n{config}")).unwrap();
    s.engine.reload().await.unwrap();
}

async fn run(s: &Server) -> (u16, RequestRecord) {
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let _ = r.bytes().await;
    for _ in 0..400 {
        if let Some(rec) = s.engine.records.get(&id)
            && rec.total_ms.is_some()
            && rec.attempts.last().is_some_and(|a| a.ended.is_some())
        {
            return (status, rec);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("record {id} never finished");
}

fn proxy_of(rec: &RequestRecord) -> Option<String> {
    rec.attempts.last().and_then(|a| a.timing.as_ref()).and_then(|t| t.proxy.clone())
}

/// A paused proxy as the engine would hold it for the current definitions.
fn pause_now(s: &Server, name: &str) {
    let print = fingerprints(&s.engine.snapshot()).remove(name).expect("a defined proxy");
    s.engine.proxy_board.pause(name, "connect to proxy failed", &print);
}

async fn op(s: &Server, v: Value) -> Value {
    nullrouter_server::operator::handle(&s.engine, &v).await
}

#[tokio::test]
async fn account_beats_provider_beats_all_and_none_stops_the_search() {
    let s = server().await;
    s.mock.respond(|_| chat_whole());
    let (a, b) = (MockProxy::start_with_auth("u", PASSWORD).await, MockProxy::start().await);
    let both = [entry("a", &a, true), entry("b", &b, false)];
    let direct = |s: &Server| s.mock.connections() - a.carried() - b.carried();

    // All providers.
    apply(&s, &both, None, "[connection]\nproxy = \"a\"\n").await;
    let (status, rec) = run(&s).await;
    assert_eq!((status, proxy_of(&rec).as_deref()), (200, Some("a")));
    assert_eq!((a.carried(), a.refused(), b.carried(), direct(&s)), (1, 0, 0, 0), "through a, with its credentials");

    // The provider's own beats all.
    apply(&s, &both, None, "[connection]\nproxy = \"a\"\n[provider.mockco.connection]\nproxy = \"b\"\n").await;
    let (_, rec) = run(&s).await;
    assert_eq!(proxy_of(&rec).as_deref(), Some("b"));
    assert_eq!((a.carried(), b.carried()), (1, 1));

    // The account's own beats the provider's.
    apply(&s, &both, Some("a"), "[provider.mockco.connection]\nproxy = \"b\"\n").await;
    let (_, rec) = run(&s).await;
    assert_eq!(proxy_of(&rec).as_deref(), Some("a"));
    assert_eq!((a.carried(), b.carried()), (2, 1));

    // `none` at the account stops the search: direct although the provider names one.
    apply(&s, &both, Some("none"), "[provider.mockco.connection]\nproxy = \"b\"\n").await;
    let (status, rec) = run(&s).await;
    assert_eq!((status, proxy_of(&rec)), (200, None));
    assert_eq!((a.carried(), b.carried(), direct(&s)), (2, 1, 1));

    // `none` at the provider stops all.
    apply(&s, &both, None, "[connection]\nproxy = \"a\"\n[provider.mockco.connection]\nproxy = \"none\"\n").await;
    let (_, rec) = run(&s).await;
    assert_eq!(proxy_of(&rec), None);
    assert_eq!((a.carried(), direct(&s)), (2, 2));
}

#[tokio::test]
async fn a_paused_proxy_sends_nothing_directly_and_leaves_no_cooldown() {
    let s = server().await;
    s.mock.respond(|_| chat_whole());
    let a = MockProxy::start().await;
    apply(&s, &[entry("a", &a, false)], Some("a"), "").await;
    pause_now(&s, "a");

    let (status, rec) = run(&s).await;
    assert_eq!(status, 503);
    match &rec.attempts[0].outcome {
        Some(AttemptOutcome::Skipped { reason, .. }) => assert_eq!(reason, "proxy a paused"),
        other => panic!("the attempt should be skipped for the proxy: {other:?}"),
    }
    assert_eq!(s.mock.connections(), 0, "nothing reached the provider, directly or through the proxy");
    assert_eq!(a.carried(), 0);

    // The pause is on every surface that is built so far.
    let live = op(&s, json!({"op": "live.snapshot"})).await;
    assert_eq!(live["paused_proxies"][0]["name"], "a", "{live}");
    let view = op(&s, json!({"op": "connection.view", "provider": "mockco"})).await;
    assert_eq!(view["providers"][0]["accounts"][0]["paused"], true, "{view}");
    let saved = std::fs::read_to_string(s.home().join("routing/proxies.json")).unwrap();
    assert!(saved.contains("\"a\"") && !saved.contains("127.0.0.1"), "names, times and reasons only: {saved}");
    assert!(ProxyBoard::open(s.home()).paused("a").is_some(), "a restart keeps the pause");

    // Fixed: the proxy answers, so the pause clears, and the model was not put to rest.
    let fixed = op(&s, json!({"op": "proxy.fixed", "name": "a"})).await;
    assert_eq!(fixed["reachable"], true, "{fixed}");
    let (status, rec) = run(&s).await;
    assert_eq!((status, proxy_of(&rec).as_deref()), (200, Some("a")));
}

#[tokio::test]
async fn fixed_with_the_proxy_still_down_keeps_the_pause_until_it_is_back() {
    let s = server().await;
    s.mock.respond(|_| chat_whole());
    let a = MockProxy::start().await;
    apply(&s, &[entry("a", &a, false)], Some("a"), "").await;
    pause_now(&s, "a");
    a.stop().await;

    let fixed = op(&s, json!({"op": "proxy.fixed", "name": "a"})).await;
    assert_eq!(fixed["reachable"], false, "{fixed}");
    assert!(fixed["reason"].is_string(), "{fixed}");
    assert!(s.engine.proxy_board.paused("a").is_some());

    a.start_again().await;
    let fixed = op(&s, json!({"op": "proxy.fixed", "name": "a"})).await;
    assert_eq!(fixed["reachable"], true, "{fixed}");
    let (status, _) = run(&s).await;
    assert_eq!(status, 200, "traffic resumes");

    let unknown = op(&s, json!({"op": "proxy.fixed", "name": "nope"})).await;
    assert_eq!(unknown["ok"], false);
    assert!(unknown["error"].as_str().unwrap().contains("known proxies: a"), "{unknown}");
}

#[tokio::test]
async fn a_dead_proxy_is_probed_and_paused_by_the_failure_itself() {
    let s = server().await;
    s.mock.respond(|_| chat_whole());
    let a = MockProxy::start().await;
    apply(&s, &[entry("a", &a, false)], Some("a"), "").await;
    a.stop().await;

    let (status, _) = run(&s).await;
    assert_ne!(status, 200);
    let paused = s.engine.proxy_board.paused("a").expect("the failed connect paused the proxy");
    assert!(paused.reason.contains("proxy"), "{}", paused.reason);
    assert_eq!(s.mock.connections(), 0, "no direct send");
}

#[tokio::test]
async fn changing_the_assignment_clears_the_pause() {
    let s = server().await;
    let (a, b) = (MockProxy::start().await, MockProxy::start().await);
    let both = [entry("a", &a, false), entry("b", &b, false)];
    apply(&s, &both, Some("a"), "").await;
    pause_now(&s, "a");

    // A reload that changes nothing about `a` keeps the pause.
    apply(&s, &both, Some("a"), "").await;
    assert!(s.engine.proxy_board.paused("a").is_some());

    // The account moves to `b`: what used `a` changed, so `a` is evaluated again.
    apply(&s, &both, Some("b"), "").await;
    assert!(s.engine.proxy_board.paused("a").is_none());
}

#[tokio::test]
async fn a_provider_error_through_a_healthy_proxy_does_not_pause_it() {
    let s = server().await;
    s.mock.respond(|_| Step::json(500, json!({"error": {"message": "boom", "type": "server_error"}})));
    let a = MockProxy::start().await;
    apply(&s, &[entry("a", &a, false)], Some("a"), "").await;

    let (status, rec) = run(&s).await;
    assert_ne!(status, 200);
    assert_eq!(proxy_of(&rec).as_deref(), Some("a"));
    assert!(s.engine.proxy_board.paused("a").is_none(), "the proxy was fine; the provider was not");
    assert_eq!(a.carried(), 1);
}

#[tokio::test]
async fn the_final_503_names_the_paused_proxy() {
    let s = server().await;
    s.mock.respond(|_| chat_whole());
    let a = MockProxy::start_with_auth("u", PASSWORD).await;
    apply(&s, &[entry("a", &a, true)], Some("a"), "").await;
    pause_now(&s, "a");

    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 503);
    let body = r.text().await.unwrap();
    assert!(body.contains("proxy a paused"), "the client is told why: {body}");
    assert!(!body.contains(PASSWORD) && !body.contains(&a.addr().to_string()), "and nothing of the proxy's address or login: {body}");
}
