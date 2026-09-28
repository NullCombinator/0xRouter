//! Real client SDKs and harnesses (T058) against the server over a scripted provider:
//! runs `tests/harness/run.sh`. Skipped unless `ZR_HARNESS=1` (needs the SDKs from
//! `tests/harness/README.md`). With headroom installed, the chain through it (T152) is
//! checked here: its marked requests reach the provider in the client's own style and are
//! left out, and recorded, across styles.

mod common;

use std::path::Path;

use common::{multi_plugin, reply_by_wire, server_with};
use zerorouter_engine::records::Query;
use zerorouter_engine::testkit::Received;

/// The chain's marker on a request that reached the provider: body field and header.
fn marker(r: &Received) -> (Option<String>, Option<String>) {
    let body = r.json()["x_chain"].as_str().map(str::to_owned);
    let header = r.headers.get("x-chain-marker").and_then(|v| v.to_str().ok()).map(str::to_owned);
    (body, header)
}

#[tokio::test(flavor = "multi_thread")]
async fn sdks_and_harnesses_accept_every_style() {
    if std::env::var("ZR_HARNESS").as_deref() != Ok("1") {
        eprintln!("skipped: set ZR_HARNESS=1 to run the harness scripts");
        return;
    }
    let s = server_with(|m| vec![multi_plugin(m)]).await;
    // Real clients send as many requests as they like, in their own order.
    s.mock.respond(reply_by_wire);
    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/harness/run.sh");
    let (base, key) = (s.base.clone(), s.key.clone());
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new("bash")
            .arg(run)
            .env("ZR_BASE", base)
            .env("ZR_KEY", key)
            .env("ZR_MODEL", "mockco/m1")
            .env("ZR_MODEL_MESSAGES", "multi/m-messages")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    println!("{text}");
    assert!(out.status.success(), "{text}");
    if text.contains("ok   headroom chain") {
        headroom_chain_kept_the_optimizers_additions(&s);
    }
}

/// Each SDK sent a whole and a streamed request to a same-style and a cross-style model.
fn headroom_chain_kept_the_optimizers_additions(s: &common::Server) {
    let got = s.mock.received();
    let arrived = |path: &str, who: &str| {
        got.iter().filter(|r| r.path_and_query.starts_with(path) && marker(r) == (Some(who.into()), Some(who.into()))).count()
    };
    assert_eq!(arrived("/multi/messages", "anthropic"), 2, "same style: the marks reach the provider");
    assert_eq!(arrived("/v1/chat/completions", "openai"), 2, "same style: the marks reach the provider");
    let crossed = got.iter().filter(|r| {
        let (b, h) = marker(r);
        (r.path_and_query.starts_with("/multi/messages") && (b.as_deref() == Some("openai") || h.as_deref() == Some("openai")))
            || (r.path_and_query.starts_with("/v1/chat") && (b.as_deref() == Some("anthropic") || h.as_deref() == Some("anthropic")))
    });
    assert_eq!(crossed.count(), 0, "cross style: no mark reaches the provider");

    let recs = s.engine.records.query(&Query::default());
    let dropped = recs
        .iter()
        .filter(|r| r.attempts.first().is_some_and(|a| a.dropped.iter().any(|d| d.path.contains("x_chain"))))
        .count();
    assert_eq!(dropped, 4, "every cross-style request records the dropped field");
}
