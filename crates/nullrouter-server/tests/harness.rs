//! Real client SDKs and harnesses (T058) against the server over a scripted provider:
//! runs `tests/harness/run.sh`. Skipped unless `NR_HARNESS=1` (needs the SDKs from
//! `tests/harness/README.md`). With headroom installed, the chain through it (T152) is
//! checked here: its marked requests reach the provider in the client's own style and are
//! left out, and recorded, across styles. Every SDK also meets an all-attempts-failed
//! request (`broken/m1`, T074), and to restarted and error-event streams (`cutco/m1`,
//! T090): the first stream for each body is cut after three words.
//!
//! Signed-in accounts (spec 005 T043, SC-002): the same SDK scripts and Claude Code run
//! against sign-in accounts on the bundled anthropic, xai and grok-cli plugins (hosts
//! pointed at the mock), one `run.sh` pass per provider.

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use common::{
    SECRET, broken_plugin, bundled_at_mock, multi_plugin, reply_by_wire, server_custom, server_with, signin_server,
};
use nullrouter_engine::keys::{self, BreakBehaviour, Keys};
use nullrouter_engine::records::{BreakHandling, Outcome, Query};
use nullrouter_engine::testkit::{CacheSim, MockUpstream, Received, Step};
use serde_json::{Value, json};

/// A provider `cutco` on the chat wire with model `m1`.
fn cut_plugin(mock: &MockUpstream) -> (&'static str, String) {
    let url = mock.url("/cutco/chat/completions");
    (
        "cutco",
        format!(
            "schema = 2\nid = \"cutco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{url}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n"
        ),
    )
}

/// "w0 w1 w2 " in chat chunks, then the connection drops.
fn cut_stream() -> Step {
    let frames = (0..3)
        .map(|i| {
            let v = json!({"id": "up-cut", "object": "chat.completion.chunk", "created": 1, "model": "m1", "choices": [{"index": 0, "delta": {"content": format!("w{i} ")}, "finish_reason": Value::Null}]});
            format!("data: {v}\n\n").into()
        })
        .collect();
    let headers = vec![("content-type".into(), "text/event-stream".into())];
    Step::Stream { status: 200, headers, frames, every: Duration::from_millis(5), cut: true }
}

/// The chain's marker on a request that reached the provider: body field and header.
fn marker(r: &Received) -> (Option<String>, Option<String>) {
    let body = r.json()["x_chain"].as_str().map(str::to_owned);
    let header = r.headers.get("x-chain-marker").and_then(|v| v.to_str().ok()).map(str::to_owned);
    (body, header)
}

/// Runs `tests/harness/run.sh` against `base` with `env` set; returns its output and status.
async fn run_sh(base: &str, key: &str, env: Vec<(&'static str, String)>) -> (String, bool) {
    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness/run.sh");
    let (base, key) = (base.to_owned(), key.to_owned());
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bash").arg(run).env("NR_BASE", base).env("NR_KEY", key).envs(env).output().unwrap()
    })
    .await
    .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (text, out.status.success())
}

#[tokio::test(flavor = "multi_thread")]
async fn sdks_and_harnesses_accept_every_style() {
    if std::env::var("NR_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipped: set NR_HARNESS=1 to run the harness scripts");
        return;
    }
    let s = server_with(|m| vec![multi_plugin(m), broken_plugin(m), cut_plugin(m)]).await;
    let mut list = Keys::load(&s.home().join(keys::FILE)).unwrap();
    let (strict, _) = list.issue("strict", Some(BreakBehaviour::ErrorEvent)).unwrap();
    list.save().unwrap();
    s.engine.reload_blocking().unwrap();
    // Real clients send as many requests as they like, in their own order. A body's
    // resent attempt after a cut is byte-identical, so it gets the whole answer.
    let seen = Mutex::new(HashSet::new());
    s.mock.respond(move |r| {
        let cut = r.path_and_query.starts_with("/cutco")
            && r.json()["stream"] == true
            && seen.lock().unwrap().insert(r.body.clone());
        if cut { cut_stream() } else { reply_by_wire(r) }
    });
    let env = vec![
        ("NR_MODEL", "mockco/m1".to_owned()),
        ("NR_MODEL_MESSAGES", "multi/m-messages".to_owned()),
        ("NR_MODEL_FAIL", "broken/m1".to_owned()),
        ("NR_MODEL_CUT", "cutco/m1".to_owned()),
        ("NR_KEY_STRICT", strict),
    ];
    let (text, ok) = run_sh(&s.base, &s.key, env).await;
    println!("{text}");
    assert!(ok, "{text}");
    // The first 401 rests the account, so the later failed requests are skipped attempts.
    assert!(s.mock.received().iter().any(|r| r.path_and_query.starts_with("/broken")), "the all-failed checks ran");
    let failed =
        s.engine.records.query(&Query::default()).iter().filter(|r| r.target.as_deref() == Some("broken/m1")).count();
    assert_eq!(failed, 16, "8 SDK scripts, a whole and a streamed request each");
    let cut = s.engine.records.query(&Query::default());
    let cut: Vec<_> = cut.iter().filter(|r| r.target.as_deref() == Some("cutco/m1")).collect();
    let restarted = cut.iter().filter(|r| r.break_handling == BreakHandling::Restarted).count();
    let ended = cut.iter().filter(|r| matches!(r.break_handling, BreakHandling::ErrorEvent { .. })).count();
    assert!(restarted >= 8 && ended >= 8, "restarted {restarted}, error events {ended}: 2 SDK scripts, 4 styles each");
    if text.contains("ok   headroom chain") {
        headroom_chain_kept_the_optimizers_additions(&s);
    }
}

/// The signed-in accounts' unified names equal the upstream ids (as in `signin_serving.rs`).
const SIGNIN_CONFIG: &str = r#"[plugin_decisions]
anthropic = "replace"
xai = "replace"
"grok-cli" = "replace"
"#;

#[tokio::test(flavor = "multi_thread")]
async fn sdks_and_claude_code_use_signed_in_accounts() {
    if std::env::var("NR_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipped: set NR_HARNESS=1 to run the harness scripts");
        return;
    }
    let s = signin_server(
        |m| ["anthropic", "xai", "grok-cli"].map(|id| (id, bundled_at_mock(m, id))).into_iter().collect(),
        &[("anthropic", "max"), ("xai", "main"), ("grok-cli", "work")],
        SIGNIN_CONFIG,
    )
    .await;
    s.mock.respond(reply_by_wire);
    let targets = [
        ("anthropic", "max", "anthropic/claude-sonnet-4-20250514"),
        ("xai", "main", "xai/grok-4"),
        ("grok-cli", "work", "grok-cli/grok-4.5"),
    ];
    for (provider, account, model) in targets {
        let before = s.mock.received().len();
        let (text, ok) = run_sh(&s.base, &s.key, vec![("NR_MODEL", model.to_owned())]).await;
        println!("== {model}\n{text}");
        assert!(ok, "{model}: {text}");
        // Every request reached the provider with the account's token, never a key.
        let got = s.mock.received();
        assert!(got.len() > before, "{model}: nothing reached the provider");
        for r in &got[before..] {
            let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
            assert_eq!(auth, format!("Bearer {SECRET}-{provider}-{account}"), "{model}: {}", r.path_and_query);
            assert!(r.headers.get("x-api-key").is_none(), "{model}: no x-api-key");
        }
    }
    // Every record (any harness, any style) succeeded on the signed-in account it named.
    let records = s.engine.records.query(&Query::default());
    assert!(!records.is_empty());
    for r in &records {
        assert_eq!(r.outcome, Outcome::Succeeded, "{r:#?}");
        let by = r.served_by.as_ref().unwrap();
        let want = targets.iter().find(|t| t.0 == by.provider).map(|t| t.1);
        assert_eq!(by.account.as_deref(), want, "{r:#?}");
    }
}

/// Each SDK sent a whole and a streamed request to a same-style and a cross-style model.
fn headroom_chain_kept_the_optimizers_additions(s: &common::Server) {
    let got = s.mock.received();
    let arrived = |path: &str, who: &str| {
        got.iter()
            .filter(|r| r.path_and_query.starts_with(path) && marker(r) == (Some(who.into()), Some(who.into())))
            .count()
    };
    assert_eq!(arrived("/multi/messages", "anthropic"), 2, "same style: the marks reach the provider");
    assert_eq!(arrived("/v1/chat/completions", "openai"), 2, "same style: the marks reach the provider");
    let crossed = got.iter().filter(|r| {
        let (b, h) = marker(r);
        (r.path_and_query.starts_with("/multi/messages")
            && (b.as_deref() == Some("openai") || h.as_deref() == Some("openai")))
            || (r.path_and_query.starts_with("/v1/chat")
                && (b.as_deref() == Some("anthropic") || h.as_deref() == Some("anthropic")))
    });
    assert_eq!(crossed.count(), 0, "cross style: no mark reaches the provider");

    let recs = s.engine.records.query(&Query::default());
    let dropped = recs
        .iter()
        .filter(|r| r.attempts.first().is_some_and(|a| a.dropped.iter().any(|d| d.path.contains("x_chain"))))
        .count();
    assert_eq!(dropped, 4, "every cross-style request records the dropped field");
}

/// `warmco`: one provider on the chat and Messages wires with a prompt cache and a window, so its
/// accounts are subscriptions. Its model `m1` sits behind the unified model `warm`.
fn warm_plugin(mock: &MockUpstream) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "warmco"
category = "apikey"
[auth]
kind = "apikey"
[[endpoints.text]]
url = "{chat}"
wire = "openai-chat"
[[endpoints.text]]
url = "{messages}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01" }}
auth = {{ header = "x-api-key", scheme = "raw" }}
[[models]]
id = "m1"
[routing.cache]
mode = "automatic"
lifetime = "5m"
min_tokens = 0
[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 100000000
"#,
        chat = mock.url("/warmco/chat/completions"),
        messages = mock.url("/warmco/messages"),
    );
    ("warmco", toml)
}

/// SC-009, warm part: two agents on two harnesses (the Python OpenAI SDK over chat, Claude Code
/// over Messages), each in a multi-turn session through a unified model behind two accounts. No
/// client error, each agent's requests stay on one account, and the provider reports cache reads
/// on every request that stayed warm.
#[tokio::test(flavor = "multi_thread")]
async fn warm_sessions_stay_on_one_account_through_real_harnesses() {
    if std::env::var("NR_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipped: set NR_HARNESS=1 to run the harness scripts");
        return;
    }
    let accounts = format!(
        "schema = 2\n[[account]]\nprovider = \"warmco\"\nname = \"one\"\nsecret = \"{SECRET}-one\"\n[[account]]\nprovider = \"warmco\"\nname = \"two\"\nsecret = \"{SECRET}-two\"\n"
    );
    let unified = "[[unified_model]]\nname = \"warm\"\nmembers = [{ provider = \"warmco\", model = \"m1\" }]\n";
    let s = server_custom(|m| vec![warm_plugin(m)], &accounts, unified).await;
    let mut list = Keys::load(&s.home().join(keys::FILE)).unwrap();
    let (second_key, _) = list.issue("second", None).unwrap();
    list.save().unwrap();
    s.engine.reload_blocking().unwrap();
    let cache = CacheSim::new(Duration::from_secs(300));
    cache.account(&format!("{SECRET}-one"), "warmco/one");
    cache.account(&format!("{SECRET}-two"), "warmco/two");
    s.mock.simulate_cache(&cache);

    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness/warm.sh");
    let (base, key) = (s.base.clone(), s.key.clone());
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bash")
            .arg(run)
            .env("NR_BASE", base)
            .env("NR_KEY", key)
            .env("NR_KEY_WARM", second_key)
            .env("NR_MODEL_WARM", "warm")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{text}");
    assert!(out.status.success(), "{text}");

    let records = s.engine.records.query(&Query::default());
    let records: Vec<_> = records.iter().filter(|r| r.target.as_deref() == Some("warm")).collect();
    assert!(!records.is_empty(), "the sessions reached the unified model");
    assert!(records.iter().all(|r| r.outcome == Outcome::Succeeded), "no client error");
    let mut by_agent: std::collections::BTreeMap<String, HashSet<String>> = Default::default();
    for r in &records {
        let agent = r.agent.as_ref().map(|a| a.key.clone()).unwrap_or_default();
        let account = r.served_by.as_ref().and_then(|b| b.account.clone()).unwrap_or_default();
        by_agent.entry(agent).or_default().insert(account);
        let stayed = r.decision.as_ref().and_then(|d| d.warm.as_ref()).is_some_and(|w| w.stayed);
        if stayed {
            let reads = r.usage.and_then(|u| u.cache_read).unwrap_or(0);
            assert!(reads > 0, "a warm request got no cache read: {:?} {:?}", r.decision, r.usage);
        }
    }
    for (agent, accounts) in &by_agent {
        assert_eq!(accounts.len(), 1, "agent {agent} moved between {accounts:?}");
    }
    assert!(records.iter().any(|r| r.decision.as_ref().and_then(|d| d.warm.as_ref()).is_some_and(|w| w.stayed)));
}

/// `subco` (subscription accounts: a prompt cache and a window) or `payco` (a key with a price) on
/// the chat and Messages wires, model `m1`.
fn cold_plugin(mock: &MockUpstream, id: &'static str, routing: &str) -> (&'static str, String) {
    let toml = format!(
        r#"schema = 2
id = "{id}"
category = "apikey"
[auth]
kind = "apikey"
[[endpoints.text]]
url = "{chat}"
wire = "openai-chat"
[[endpoints.text]]
url = "{messages}"
wire = "anthropic-messages"
headers = {{ "anthropic-version" = "2023-06-01" }}
auth = {{ header = "x-api-key", scheme = "raw" }}
[[models]]
id = "m1"
{routing}"#,
        chat = mock.url(&format!("/{id}/chat/completions")),
        messages = mock.url(&format!("/{id}/messages")),
    );
    (id, toml)
}

/// SC-009, cold part: the Python OpenAI SDK (cold one-shots and an overflow burst), Claude Code
/// (two cold one-shots) and the Node SDK's standing smoke through a unified model behind three
/// subscription accounts, each limited after two requests, and one pay-as-you-go key. No client
/// error, and the key serves only what no subscription could.
#[tokio::test(flavor = "multi_thread")]
async fn cold_work_and_overflow_through_real_harnesses() {
    if std::env::var("NR_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipped: set NR_HARNESS=1 to run the harness scripts");
        return;
    }
    let subs = "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.window]]\nname = \"5h\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 100000000\n";
    let pay = "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.price]]\ninput = 2.0\noutput = 8.0\n";
    let mut accounts = String::from("schema = 2\n");
    for n in ["s1", "s2", "s3"] {
        accounts += &format!("[[account]]\nprovider = \"subco\"\nname = \"{n}\"\nsecret = \"{SECRET}-{n}\"\n");
    }
    accounts += &format!("[[account]]\nprovider = \"payco\"\nname = \"key\"\nsecret = \"{SECRET}-key\"\n");
    let unified = "[[unified_model]]\nname = \"cold\"\nmembers = [{ provider = \"subco\", model = \"m1\" }, { provider = \"payco\", model = \"m1\" }]\n";
    let s =
        server_custom(|m| vec![cold_plugin(m, "subco", subs), cold_plugin(m, "payco", pay)], &accounts, unified).await;
    let cache = CacheSim::new(Duration::from_secs(300));
    for n in ["s1", "s2", "s3"] {
        cache.account(&format!("{SECRET}-{n}"), &format!("subco/{n}"));
        // Each subscription takes two requests and then answers 429 for the rest of the run.
        cache.rate_limit(&format!("subco/{n}"), 3, Duration::from_secs(86_400));
    }
    cache.account(&format!("{SECRET}-key"), "payco/key");
    s.mock.simulate_cache(&cache);

    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness/cold.sh");
    let (base, key) = (s.base.clone(), s.key.clone());
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bash")
            .arg(run)
            .env("NR_BASE", base)
            .env("NR_KEY", key)
            .env("NR_MODEL_COLD", "cold")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{text}");
    assert!(out.status.success(), "{text}");

    let records = s.engine.records.query(&Query::default());
    let records: Vec<_> = records.iter().filter(|r| r.target.as_deref() == Some("cold")).collect();
    assert!(!records.is_empty(), "the clients reached the unified model");
    assert!(records.iter().all(|r| r.outcome == Outcome::Succeeded), "no client error");
    let by = |provider: &str| {
        records.iter().filter(|r| r.served_by.as_ref().is_some_and(|b| b.provider == provider)).count()
    };
    assert!(by("subco") >= 6, "the subscriptions took their share first: {}", by("subco"));
    assert!(by("payco") >= 1, "the burst was more than the subscriptions take");
    // The key served only what no subscription could: every subscription was resting or had
    // already failed this very request.
    for r in records.iter().filter(|r| r.served_by.as_ref().is_some_and(|b| b.provider == "payco")) {
        let d = r.decision.as_ref().expect("a decision");
        let subs_open = d.candidates.iter().any(|c| c.provider == "subco" && c.eligible);
        assert!(!subs_open || r.attempts.len() > 1, "pay-as-you-go served while a subscription could: {d:?}");
    }
}
