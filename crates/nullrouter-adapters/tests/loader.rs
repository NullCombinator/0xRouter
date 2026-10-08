//! Loading the serving version of a third-party adapter from the store (T046).

use std::fs;
use std::sync::Arc;

use nullrouter_adapters::HarnessName;
use nullrouter_adapters::alerts::{AlertKind, AlertLog};
use nullrouter_adapters::fingerprint;
use nullrouter_adapters::loader::Loader;
use nullrouter_adapters::record::NotRunReason;
use nullrouter_adapters::store::{Origin, Store, VersionEntry, VersionId, VersionState};
use nullrouter_sandbox::{SandboxEngine, wasm_hash};

const MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["messages[*].reasoning_content"]
"#;
const LIB: &str = "// the adapter's source";

struct Fixture {
    _home: tempfile::TempDir,
    store: Store,
    alerts: Arc<AlertLog>,
    loader: Loader,
    name: HarnessName,
    id: VersionId,
}

fn good_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
            (@custom "nr.abi" "\01\00\00\00"))"#,
    )
    .unwrap()
}

/// A module with no `nr.abi` section, which the load gate refuses.
fn gateless_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0))"#,
    )
    .unwrap()
}

/// An approved version of `acme` with `wasm` as its module.
fn approved(wasm: &[u8]) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let name = HarnessName::new("acme").unwrap();
    let files =
        [("adapter.toml".to_owned(), MANIFEST.as_bytes().to_vec()), ("src/lib.rs".to_owned(), LIB.as_bytes().to_vec())];
    let fp = fingerprint::of_files(&files);
    let semver = semver::Version::new(1, 0, 0);
    let id = VersionId::new(&semver, &fp);
    for (path, bytes) in &files {
        store.write_version_file(&name, &id, &format!("source/{path}"), bytes).unwrap();
    }
    store.write_version_file(&name, &id, "module.wasm", wasm).unwrap();

    let mut index = store.load_index().unwrap();
    let entry = VersionEntry {
        id: id.clone(),
        semver: semver.to_string(),
        state: VersionState::Queued,
        source_fp: fp,
        wasm_hash: Some(wasm_hash(wasm)),
        kit_abi: Some(1),
        submitted: "2026-10-08T00:00:00Z".into(),
        state_reason: String::new(),
        rebuilding: false,
        rebuild_failed: false,
        origin: Origin::Local("test".into()),
    };
    index.submit(&name, entry).unwrap();
    for to in [VersionState::Building, VersionState::InReview, VersionState::Reported] {
        index.transition(&name, &id, to, "").unwrap();
    }
    index.approve(&name, &id).unwrap();
    store.save_index(&index).unwrap();

    let alerts = Arc::new(AlertLog::open(&store));
    let sandbox = Arc::new(SandboxEngine::new(4).unwrap());
    let loader = Loader::new(store.clone(), alerts.clone(), sandbox, Arc::new(|s: &str| s.to_owned()));
    Fixture { _home: home, store, alerts, loader, name, id }
}

fn kinds(f: &Fixture) -> Vec<AlertKind> {
    f.alerts.list().unwrap().iter().map(|a| a.kind).collect()
}

#[test]
fn the_serving_version_is_loaded() {
    let f = approved(&good_module());
    let h = f.loader.handle(&f.name);
    assert!(h.module.is_some());
    assert_eq!((h.harness.as_str(), h.version.as_str()), ("acme", f.id.as_str()));
    assert!(kinds(&f).is_empty());
}

#[test]
fn a_harness_the_store_does_not_know_has_no_approved_version() {
    let f = approved(&good_module());
    let h = f.loader.handle(&HarnessName::new("other").unwrap());
    assert!(h.module.is_none());
    assert_eq!(h.reason, NotRunReason::NoApprovedVersion);
}

#[test]
fn a_suspect_version_does_not_serve() {
    let f = approved(&good_module());
    let mut index = f.store.load_index().unwrap();
    index.transition(&f.name, &f.id, VersionState::Suspect, "guardrail").unwrap();
    f.store.save_index(&index).unwrap();
    let h = f.loader.handle(&f.name);
    assert!(h.module.is_none());
    assert_eq!(h.reason, NotRunReason::Suspect);
    assert_eq!(h.version, f.id.as_str());
}

#[test]
fn an_edited_source_is_not_loaded_and_raises_an_alert() {
    let f = approved(&good_module());
    let path = f.store.version_dir(&f.name, &f.id).join("source/src/lib.rs");
    fs::write(path, "// edited after approval").unwrap();
    let h = f.loader.handle(&f.name);
    assert!(h.module.is_none());
    assert_eq!(h.reason, NotRunReason::SourceMismatch);
    assert_eq!(kinds(&f), [AlertKind::SourceMismatch]);
}

#[test]
fn an_edited_module_is_not_loaded_and_raises_an_alert() {
    let f = approved(&good_module());
    let path = f.store.version_dir(&f.name, &f.id).join("module.wasm");
    fs::write(path, gateless_module()).unwrap();
    let h = f.loader.handle(&f.name);
    assert!(h.module.is_none());
    assert_eq!(h.reason, NotRunReason::SourceMismatch);
    assert_eq!(kinds(&f), [AlertKind::SourceMismatch]);
}

#[test]
fn a_module_the_load_gate_refuses_is_not_loaded_and_raises_an_alert() {
    let f = approved(&gateless_module());
    let h = f.loader.handle(&f.name);
    assert!(h.module.is_none());
    assert_eq!(h.reason, NotRunReason::NoApprovedVersion);
    assert_eq!(kinds(&f), [AlertKind::ModuleRefused]);
}

#[test]
fn a_second_load_reuses_the_compiled_module() {
    let f = approved(&good_module());
    let (a, b) = (f.loader.handle(&f.name), f.loader.handle(&f.name));
    assert!(Arc::ptr_eq(&a.module.as_ref().unwrap().module, &b.module.as_ref().unwrap().module));
}
