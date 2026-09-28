//! Live checks against the real providers (T060), run by the operator with their own
//! accounts: `ZR_LIVE=1 cargo test -p zerorouter-engine --test live -- text`. Skipped
//! unless `ZR_LIVE=1`. The accounts come from `$ZEROROUTER_HOME` (else `~/.0router`); a
//! provider without an account is skipped with a message.
//!
//! `ZR_LIVE_MODELS` (comma-separated `provider/model`) replaces the default targets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{self, Answer, TextRequest};
use zerorouter_engine::keys::AgentId;
use zerorouter_engine::records::{Outcome, RequestRecord};
use zerorouter_engine::state::Engine;
use zerorouter_registry::OperatorHome;
use zerorouter_wire::codec::{request, response};
use zerorouter_wire::codec::response::ForClient;

/// One cheap target per text provider and wire. opencode's Messages wire takes `x-api-key`
/// and neither opencode wire gets fingerprint tools (R4): these requests carry no tools.
const TARGETS: &[&str] = &[
    "anthropic/claude-sonnet-4-20250514",
    "openrouter/openai/gpt-4o-mini",
    "opencode-zen/gpt-5-nano",
    "opencode-zen/claude-haiku-4-5",
    "opencode-go/deepseek-flash",
];

fn live() -> bool {
    std::env::var("ZR_LIVE").is_ok_and(|v| v == "1")
}

fn targets() -> Vec<String> {
    match std::env::var("ZR_LIVE_MODELS") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(|s| s.trim().to_owned()).collect(),
        _ => TARGETS.iter().map(|s| (*s).to_owned()).collect(),
    }
}

async fn settled(engine: &Engine, id: &str) -> RequestRecord {
    for _ in 0..600 {
        let r = engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("record {id} never finished");
}

/// Sends one request in `client`'s style and returns the answer's text and the record.
async fn send(engine: &Arc<Engine>, client: &str, target: &str, body: Value) -> (String, RequestRecord) {
    let st = engine.snapshot();
    let style = st.style(client).unwrap().clone();
    let ir = request::decode(&style, &body).unwrap();
    let id = zerorouter_engine::records::new_id();
    engine.records.insert(RequestRecord::new(id.clone(), "live".into(), style.id.clone()));
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let req = TextRequest {
        id: id.clone(),
        arrived: Instant::now(),
        client: style.clone(),
        body: body.clone(),
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_live", Some("live-1")),
        target: target.into(),
        stream,
        cancel: CancellationToken::new(),
    };
    let text = match engine.text(st.clone(), req).await {
        Ok(Answer::Whole { status, raw, answer, .. }) => {
            assert_eq!(status, 200, "{target}: {}", String::from_utf8_lossy(&raw));
            match *answer {
                ForClient::AsReceived { read } => read.or_else(|| response::decode(&style, &serde_json::from_slice(&raw).ok()?).ok()).map(|r| r.text()).unwrap_or_default(),
                ForClient::Rebuilt { read, .. } => read.text(),
            }
        }
        Ok(Answer::Events { rx, .. }) => match attempt::collect(&style, &body, rx).await {
            Ok(r) => r.text(),
            Err(e) => panic!("{target}: the stream ended in an error: {e:?}"),
        },
        Err(f) => panic!("{target}: {} {}", f.status, f.message),
    };
    (text, settled(engine, &id).await)
}

#[tokio::test]
async fn text() {
    if !live() {
        eprintln!("skipped: set ZR_LIVE=1 to run the live checks");
        return;
    }
    let (engine, report) = Engine::open(OperatorHome::resolve()).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    let engine = Arc::new(engine);
    let st = engine.snapshot();
    let mut ran = 0;
    for target in targets() {
        let provider = target.split('/').next().unwrap();
        if st.accounts.for_provider(provider).all(|a| a.disabled) {
            eprintln!("{target}: skipped, no enabled {provider} account");
            continue;
        }
        for stream in [false, true] {
            let body = json!({
                "model": target,
                "max_tokens": 64,
                "stream": stream,
                "messages": [{ "role": "user", "content": "Reply with the single word: pong" }],
            });
            let started = Instant::now();
            let (text, rec) = send(&engine, "openai-chat", &target, body).await;
            let (input, output) = rec.usage.map_or((None, None), |u| (u.input, u.output));
            eprintln!(
                "{target} stream={stream}: {:?} in {} ms, input {input:?} output {output:?}, {} attempt(s)",
                text.trim(),
                started.elapsed().as_millis(),
                rec.attempts.len()
            );
            assert_eq!(rec.outcome, Outcome::Succeeded, "{target} stream={stream}: {rec:#?}");
            assert!(!text.trim().is_empty(), "{target} stream={stream}: an empty answer");
            assert!(input.is_some() && output.is_some(), "{target} stream={stream}: usage not recorded: {rec:#?}");
        }
        ran += 1;
    }
    assert!(ran > 0, "no provider had an account under {}", engine.home().path().display());
}
