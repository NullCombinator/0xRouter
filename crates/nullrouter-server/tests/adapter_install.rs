//! The install pipeline end to end (spec 004, T053): `install` of the noop fixture through the
//! real builder, a mock review model, and `approve`. Needs the builder binary and the wasm32
//! target, and skips with a message where either is missing (CI has both).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::{Server, chat_whole, chat_whole_text, server};
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::store::{ReviewConfig, Store, VersionId, VersionState};
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::records::{AdapterOutcome, NotRunReason, RequestRecord};
use nullrouter_server::relay::REQUEST_ID;
use serde_json::json;

const GOOD: &str = r#"{"risk":"low","summary":"changes nothing","findings":[]}"#;

fn wasm_target_installed() -> bool {
    let args = ["target", "list", "--installed", "--toolchain", nullrouter_builder::TOOLCHAIN];
    let Ok(out) = Command::new("rustup").args(args).output() else { return false };
    out.status.success()
        && String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == nullrouter_builder::TARGET)
}

/// The builder binary: `$NR_BUILDER`, else `nullrouter-builder` next to the test binaries. Cargo
/// builds it for `cargo test --workspace` but not for a test of this one crate.
fn builder_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NR_BUILDER").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let found = exe.parent()?.parent()?.join("nullrouter-builder");
    found.is_file().then_some(found)
}

fn noop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../nullrouter-adapters/tests/fixtures/noop")
}

fn harness(name: &str) -> HarnessName {
    HarnessName::new(name).unwrap()
}

fn state_of(home: &Path, id: &VersionId) -> VersionState {
    let index = Store::open(home).unwrap().load_index().unwrap();
    index.version(&harness("noop"), id).expect("the version is in the index").state
}

/// One chat request with `key`; returns the settled record.
async fn ask(s: &Server, key: &str) -> RequestRecord {
    s.mock.push([chat_whole()]);
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(key)
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

fn run_of(rec: &RequestRecord) -> AdapterOutcome {
    rec.attempts[0].adapter.as_ref().expect("the attempt records the adapter run").outcome.clone()
}

#[tokio::test]
async fn install_review_approve_and_the_bound_key_starts_running() {
    if !wasm_target_installed() {
        eprintln!("skipping: wasm32-unknown-unknown for {} is not installed", nullrouter_builder::TOOLCHAIN);
        return;
    }
    let Some(builder) = builder_binary() else {
        eprintln!("skipping: the nullrouter-builder binary is not built (cargo build -p nullrouter-builder)");
        return;
    };
    let s = server().await;
    let home = s.home().to_owned();
    nullrouter_builder::setup(&nullrouter_builder::builder_dir(&home)).expect("the builder is set up");

    // The review model is the mock; the engine reads the index for it.
    let store = Store::open(&home).unwrap();
    let mut index = store.load_index().unwrap();
    index.review = Some(ReviewConfig { model: "mockco/m1".into(), budget_tokens: 200_000, reserve_output: 1024 });
    store.save_index(&index).unwrap();

    // `laptop` is bound to noop, `other` to a harness with nothing installed.
    s.engine.open_adapters().unwrap();
    let mut keys = Keys::load(&home.join(keys::FILE)).unwrap();
    keys.set_adapter("laptop", Some(harness("noop"))).unwrap();
    let (other, _) = keys.issue("other", None).unwrap();
    keys.set_adapter("other", Some(harness("zeta"))).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();

    s.mock.push([chat_whole_text(GOOD)]);
    let done = s.engine.install_adapter(&noop(), builder.to_str()).await.expect("the install runs");
    assert_eq!(done.harness, harness("noop"));
    assert_eq!(done.state, VersionState::InReview, "{done:?}");

    // queued -> building -> in_review is behind us; the queue takes it to reported.
    let mut state = state_of(&home, &done.version);
    for _ in 0..600 {
        if state == VersionState::Reported {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        state = state_of(&home, &done.version);
    }
    assert_eq!(state, VersionState::Reported);
    let dir = store.version_dir(&harness("noop"), &done.version);
    for file in ["source/adapter.toml", "module.wasm", "build.json", "review.json"] {
        assert!(dir.join(file).is_file(), "{file} was not stored");
    }

    // Until approval both keys are plain clients.
    let rec = ask(&s, &s.key).await;
    assert_eq!(run_of(&rec), AdapterOutcome::NotRun { reason: NotRunReason::NoApprovedVersion });

    nullrouter_adapters::approve(&home, &harness("noop"), &done.version, Some("looks fine")).unwrap();
    assert!(dir.join("decision.json").is_file());
    s.engine.refresh_adapters();

    // From the next request on the adapter runs; the other harness's key is unaffected.
    let rec = ask(&s, &s.key).await;
    assert_eq!(run_of(&rec), AdapterOutcome::Ran, "{:?}", rec.attempts[0].adapter);
    let rec = ask(&s, &other).await;
    assert_eq!(run_of(&rec), AdapterOutcome::NotRun { reason: NotRunReason::NoApprovedVersion });
}

#[tokio::test]
async fn a_missing_builder_leaves_the_version_queued() {
    let s = server().await;
    s.engine.open_adapters().unwrap();
    let done = s.engine.install_adapter(&noop(), Some("/nonexistent/nullrouter-builder")).await.unwrap();
    assert_eq!(done.state, VersionState::Queued);
    assert_eq!(done.reason, "builder_not_installed");
    assert_eq!(state_of(s.home(), &done.version), VersionState::Queued);
}
