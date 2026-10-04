//! Stay-warm under fingerprint semantics (spec 006, T021, SC-003): an agent keeps the account
//! that holds its prompt prefix until that account can't serve, and a success is only counted
//! when the answer completes. Slice 003's "last account that served" map is gone; the
//! scenarios that still hold are kept, the rest (last success wins between two requests in
//! flight) are replaced by the prefix fingerprints, which every success adds to.

mod common;

use std::time::Duration;

use common::*;
use nullrouter_engine::attempt::Answer;
use nullrouter_engine::classify;
use nullrouter_engine::records::AttemptKind;
use nullrouter_engine::testkit::Step;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const ROUTING: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "5m"
min_tokens = 0

[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
"#;

fn fleet() -> Fleet {
    Fleet::new()
        .provider("alpha", ROUTING, &[("5h", "tokens")])
        .account("alpha", "main", 1.0)
        .account("alpha", "backup", 1.0)
}

fn chat(turns: &[&str], stream: bool) -> serde_json::Value {
    let mut messages = vec![json!({"role": "system", "content": "You answer briefly and never guess."})];
    for (i, t) in turns.iter().enumerate() {
        messages.push(json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": t}));
    }
    json!({"model": "alpha/m1", "stream": stream, "messages": messages})
}

async fn send_turns(f: &FleetSetup, turns: &[&str]) -> String {
    let req = request(&f.setup, "openai-chat", "alpha/m1", chat(turns, false), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    assert!(f.setup.engine.text(f.setup.engine.snapshot(), req).await.is_ok());
    id
}

#[tokio::test]
async fn the_agent_stays_on_backup_until_backup_cant_serve_then_returns_to_main() {
    let f = fleet().build().await;
    // main rests for 2 s after a 429, so the agent lands on backup.
    f.setup.engine.cooldowns.fail("alpha", "main", "m1", &classify::upstream(429, "slow down"));
    let id = send_turns(&f, &["a"]).await;
    assert_eq!(trail(&f.setup.engine.records.get(&id).unwrap()), [t("alpha", "backup", AttemptKind::Initial)]);

    // Main is back, and first in operator order: the warm account still wins.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let id = send_turns(&f, &["a", "b", "c"]).await;
    assert_eq!(trail(&f.setup.engine.records.get(&id).unwrap()), [t("alpha", "backup", AttemptKind::Initial)]);

    // Backup is rate limited: the request moves to main, which now holds the newer prefix.
    f.setup.engine.cooldowns.fail("alpha", "backup", "m1", &classify::upstream(429, "slow down"));
    let id = send_turns(&f, &["a", "b", "c", "d", "e"]).await;
    assert_eq!(trail(&f.setup.engine.records.get(&id).unwrap()), [t("alpha", "main", AttemptKind::Initial)]);
    assert_eq!(accounts_hit(&f.setup), ["alpha-backup", "alpha-backup", "alpha-main"]);
}

#[tokio::test]
async fn a_cooling_warm_account_is_passed_over_for_one_that_can_serve() {
    let f = fleet().build().await;
    send_turns(&f, &["a"]).await;
    f.setup.engine.cooldowns.fail("alpha", "main", "m1", &classify::upstream(500, "boom"));
    let id = send_turns(&f, &["a", "b", "c"]).await;
    // The resting account is never tried; the request goes to the account that can serve.
    assert_eq!(trail(&f.setup.engine.records.get(&id).unwrap()), [t("alpha", "backup", AttemptKind::Initial)]);
    assert_eq!(accounts_hit(&f.setup), ["alpha-main", "alpha-backup"]);
}

#[tokio::test]
async fn a_stream_that_breaks_isnt_counted_as_warm() {
    // The break ends the request with an error event instead of resuming it.
    let f = fleet().config("[pipeline]\nbreak_behaviour = \"error_event\"\n").build().await;
    // Frames spaced out, so the cut can't overtake them.
    let Step::Stream { status, headers, frames, .. } = chat_chunks().cut_after(2) else { unreachable!() };
    f.setup.mock.push([Step::Stream { status, headers, frames, every: Duration::from_millis(20), cut: true }]);
    let req = request(&f.setup, "openai-chat", "alpha/m1", chat(&["a"], true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Answer::Events { mut rx, .. } = f.setup.engine.text(f.setup.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    drain(&mut rx).await;
    settled(&f.setup, &id).await;

    // Nothing was cached for the agent: the next request is cold.
    let id = send_turns(&f, &["a", "b", "c"]).await;
    let record = settled(&f.setup, &id).await;
    assert!(record.decision.unwrap().warm.is_none());
}
