//! Third-party adapters from the adapter store, through the engine (spec 004, T046, T036).

mod common;

use std::os::unix::fs::PermissionsExt;

use common::*;
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::alerts::{AlertKind, AlertLog};
use nullrouter_adapters::store::{Store, VersionState};
use nullrouter_adapters::testkit::install_fixture;
use nullrouter_engine::keys::Keys;
use nullrouter_engine::records::{AdapterOutcome, NotRunReason, RequestRecord};
use nullrouter_sandbox::wasm_hash;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["messages[*].reasoning_content"]
"#;
const OUT_AT: i64 = 32768;

/// A module that removes `messages[1].reasoning_content`.
fn remover() -> Vec<u8> {
    answering(&json!({"edits": [{"op": "remove", "path": "messages[1].reasoning_content",
                                "kind": "removed", "reason": "target_rejects_field"}]}))
}

/// `text` as the body of a WAT data string: quotes, backslashes and non-printing bytes by hex.
fn escape(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b' '..=b'~' if b != b'"' && b != b'\\' => (b as char).to_string(),
            _ => format!("\\{b:02x}"),
        })
        .collect()
}

/// A module that answers `answer` on every request.
fn answering(answer: &Value) -> Vec<u8> {
    let out = answer.to_string();
    wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const {OUT_AT}) "{}")
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const {})
            (@custom "nr.abi" "\01\00\00\00"))"#,
        escape(&out),
        (OUT_AT << 32) | out.len() as i64
    ))
    .unwrap()
}

async fn alpha() -> Setup {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    s.mock.push([ok()]);
    s
}

fn key(s: &Setup, harness: &str) -> String {
    let path = s._dir.path().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    let id = keys.issue("k0", None).unwrap().1.id.clone();
    keys.set_harness(&id, Some(nullrouter_engine::keys::HarnessName::new(harness).unwrap())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
    id
}

fn body() -> Value {
    json!({"model": "alpha/m1", "stream": false, "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "hello", "reasoning_content": "SENTINEL-REASONING"}
    ]})
}

async fn send(s: &Setup, agent: &str) -> RequestRecord {
    let req = request(s, "openai-chat", "alpha/m1", body(), agent, CancellationToken::new());
    let id = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.unwrap();
    settled(s, &id).await
}

#[tokio::test]
async fn an_approved_version_in_the_store_runs_on_its_harness_keys() {
    let s = alpha().await;
    install_fixture(s._dir.path(), "acme", MANIFEST, &remover());
    s.engine.open_adapters().unwrap();
    let id = key(&s, "acme");
    let rec = send(&s, &id).await;

    let sent = String::from_utf8_lossy(&s.mock.received()[0].body).to_string();
    assert!(!sent.contains("SENTINEL-REASONING"), "{sent}");
    let run = rec.attempts[0].adapter.as_ref().expect("the attempt records the run");
    assert_eq!(run.outcome, AdapterOutcome::Ran, "{run:?}");
    assert_eq!(run.harness, "acme");
}

#[tokio::test]
async fn a_version_edited_after_approval_runs_as_a_plain_client_and_the_reload_notices() {
    let s = alpha().await;
    let wasm = remover();
    install_fixture(s._dir.path(), "acme", MANIFEST, &wasm);
    s.engine.open_adapters().unwrap();
    let id = key(&s, "acme");

    // Edit the module on disk, then reload: the hash no longer matches the index.
    let adapters = s._dir.path().join("adapters/acme");
    let version = std::fs::read_dir(&adapters).unwrap().next().unwrap().unwrap().path();
    std::fs::write(version.join("module.wasm"), b"\0asm\x01\0\0\0").unwrap();
    assert_ne!(wasm_hash(&std::fs::read(version.join("module.wasm")).unwrap()), wasm_hash(&wasm));
    s.engine.reload_blocking().unwrap();

    let rec = send(&s, &id).await;
    assert!(String::from_utf8_lossy(&s.mock.received()[0].body).contains("SENTINEL-REASONING"));
    let run = rec.attempts[0].adapter.as_ref().unwrap();
    assert_eq!(run.outcome, AdapterOutcome::NotRun { reason: NotRunReason::SourceMismatch });
}

#[tokio::test]
async fn a_group_readable_adapters_directory_is_refused() {
    let s = alpha().await;
    let dir = s._dir.path().join("adapters");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
    let err = s.engine.open_adapters().unwrap_err().to_string();
    assert!(err.contains("chmod 700"), "{err}");
}

const TOOL_MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["messages[*].tool_calls"]
"#;

fn body_with_tool_call() -> Value {
    json!({"model": "alpha/m1", "stream": false, "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "checking", "tool_calls": [
            {"id": "c1", "type": "function",
             "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}}]}
    ]})
}

#[tokio::test]
async fn a_blocked_run_marks_the_version_suspect_raises_an_alert_and_stops_it_serving() {
    let s = alpha().await;
    s.mock.push([ok()]);
    let changed = json!({"edits": [{"op": "replace", "path": "messages[1].tool_calls", "kind": "converted",
        "reason": "format_conversion", "value": [{"id": "c1", "type": "function",
        "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]}]});
    install_fixture(s._dir.path(), "acme", TOOL_MANIFEST, &answering(&changed));
    s.engine.open_adapters().unwrap();
    let id = key(&s, "acme");

    let send_tool = |agent: String| {
        let req = request(&s, "openai-chat", "alpha/m1", body_with_tool_call(), &agent, CancellationToken::new());
        async {
            let rid = req.id.clone();
            s.engine.text(s.engine.snapshot(), req).await.unwrap();
            settled(&s, &rid).await
        }
    };
    let first = send_tool(id.clone()).await;
    let run = first.attempts[0].adapter.as_ref().unwrap();
    assert_eq!(run.outcome, AdapterOutcome::Blocked, "{run:?}");
    assert!(String::from_utf8_lossy(&s.mock.received()[0].body).contains("Oslo"), "the original went on");

    let store = Store::open(s._dir.path()).unwrap();
    let alerts = AlertLog::open(&store).list().unwrap();
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert_eq!(alerts[0].kind, AlertKind::Guardrail);
    assert_eq!(alerts[0].record.as_deref(), Some(first.id.as_str()));
    assert!(!alerts[0].detail.contains("Paris"), "{}", alerts[0].detail);
    let index = store.load_index().unwrap();
    let h = index.harness(&HarnessName::new("acme").unwrap()).unwrap();
    assert_eq!(h.versions[0].state, VersionState::Suspect);

    // The next request finds the version suspect and runs as a plain client.
    let second = send_tool(id).await;
    let run = second.attempts[0].adapter.as_ref().unwrap();
    assert_eq!(run.outcome, AdapterOutcome::NotRun { reason: NotRunReason::Suspect }, "{run:?}");
}
