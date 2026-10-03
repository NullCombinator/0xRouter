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

use common::{SECRET, broken_plugin, bundled_at_mock, multi_plugin, reply_by_wire, server_with, signin_server};
use nullrouter_engine::keys::{self, BreakBehaviour, Keys};
use nullrouter_engine::records::{BreakHandling, Outcome, Query};
use nullrouter_engine::testkit::{MockUpstream, Received, Step};
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
