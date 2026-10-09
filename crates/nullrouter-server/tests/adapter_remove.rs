//! Removing an adapter (spec 004, US4, T075): the bound keys become plain clients recorded
//! `not_run{removed}`, and the active version needs `--force`. This drives the library `remove`
//! the CLI calls; the CLI's listing and confirmation are the CLI's own.

mod common;

use std::time::Duration;

use common::{Server, chat_whole, server};
use nullrouter_adapters::record::{AdapterOutcome, NotRunReason};
use nullrouter_adapters::store::{Store, VersionState};
use nullrouter_adapters::testkit::{Behaviour, install_fixture, wat_adapter};
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::RequestRecord;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::json;

const MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["model"]

[response]
selectors = ["choices[*].delta"]
events = true
"#;

fn acme() -> HarnessName {
    HarnessName::new("acme").unwrap()
}

/// Installs `acme` and binds the server's key to it.
fn install(s: &Server) -> nullrouter_adapters::store::VersionId {
    let id = install_fixture(s.home(), "acme", MANIFEST, &wat_adapter(Behaviour::NoEdits));
    s.engine.open_adapters().unwrap();
    let mut keys = Keys::load(&s.home().join("keys.toml")).unwrap();
    keys.set_adapter("laptop", Some(acme())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
    id
}

async fn ask(s: &Server) -> RequestRecord {
    s.mock.push([chat_whole()]);
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .header("content-type", "application/json")
        .body(json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let _ = r.text().await.unwrap();
    for _ in 0..200 {
        if let Some(rec) = s.engine.records.get(&id).filter(|r| r.total_ms.is_some()) {
            return rec;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the record never settled: {:?}", s.engine.records.get(&id));
}

fn outcome(rec: &RequestRecord) -> AdapterOutcome {
    rec.attempts[0].adapter.as_ref().expect("the attempt records the adapter run").outcome.clone()
}

#[tokio::test]
async fn removing_the_harness_turns_its_keys_into_plain_clients_recorded_removed() {
    let s = server().await;
    install(&s);
    assert_eq!(outcome(&ask(&s).await), AdapterOutcome::Ran);

    let done = nullrouter_adapters::remove(s.home(), &acme(), None, true).unwrap();
    assert_eq!(done.versions.len(), 1);
    s.engine.refresh_adapters();

    let rec = ask(&s).await;
    assert_eq!(outcome(&rec), AdapterOutcome::NotRun { reason: NotRunReason::Removed }, "{rec:?}");
    // The key stays bound: removal changes the adapter, not the key.
    let keys = Keys::load(&s.home().join("keys.toml")).unwrap();
    assert_eq!(keys.iter().find(|k| k.name == "laptop").unwrap().adapter, Some(acme()));
    // The files went with it.
    let dirs = std::fs::read_dir(s.home().join("adapters").join("acme")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(dirs, 0, "the version directories are deleted");
}

#[tokio::test]
async fn the_active_version_is_refused_without_force_and_nothing_changes() {
    let s = server().await;
    let id = install(&s);
    let err = nullrouter_adapters::remove(s.home(), &acme(), Some(&id), false).unwrap_err();
    assert!(err.to_string().contains("--force"), "{err}");

    let index = Store::open(s.home()).unwrap().load_index().unwrap();
    assert_eq!(index.version(&acme(), &id).unwrap().state, VersionState::Approved);
    assert!(s.home().join("adapters/acme").join(id.as_str()).is_dir());
    s.engine.refresh_adapters();
    assert_eq!(outcome(&ask(&s).await), AdapterOutcome::Ran);

    nullrouter_adapters::remove(s.home(), &acme(), Some(&id), true).unwrap();
    s.engine.refresh_adapters();
    let rec = ask(&s).await;
    assert_eq!(outcome(&rec), AdapterOutcome::NotRun { reason: NotRunReason::Removed }, "{rec:?}");
}

#[tokio::test]
async fn a_version_that_is_not_active_goes_without_force() {
    let s = server().await;
    let first = install(&s);
    // A second approved version supersedes the first and becomes the active one.
    let manifest = format!("{MANIFEST}\n# v2\n");
    let second = install_fixture(s.home(), "acme", &manifest, &wat_adapter(Behaviour::NoEdits));
    assert_ne!(first, second);

    let done = nullrouter_adapters::remove(s.home(), &acme(), Some(&first), false).unwrap();
    assert_eq!(done.versions, vec![first.clone()]);
    let index = Store::open(s.home()).unwrap().load_index().unwrap();
    assert!(index.version(&acme(), &first).is_none());
    assert_eq!(index.version(&acme(), &second).unwrap().state, VersionState::Approved);
    s.engine.refresh_adapters();
    assert_eq!(outcome(&ask(&s).await), AdapterOutcome::Ran, "the active version keeps serving");

    let unknown = nullrouter_adapters::remove(s.home(), &acme(), Some(&first), false).unwrap_err();
    assert!(unknown.to_string().contains("no version"), "{unknown}");
}
