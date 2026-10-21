//! Kit-upgrade rebuild (T073, FR-032, SC-013): an approved version built for an ABI the sandbox
//! no longer runs is rebuilt from its stored source through a stub builder.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nullrouter_adapters::alerts::{AlertKind, AlertLog};
use nullrouter_adapters::loader::Loader;
use nullrouter_adapters::record::NotRunReason;
use nullrouter_adapters::store::{Origin, Store, VersionEntry, VersionId, VersionState};
use nullrouter_adapters::{fingerprint, startup_rebuilds, HarnessName, InstallOptions};
use nullrouter_sandbox::{wasm_hash, SandboxEngine};

const MANIFEST: &str = r#"
harness = "acme"
style = "openai-chat"
kit = "1"

[request]
selectors = ["messages[*].reasoning_content"]
"#;
const LIB: &str = "// the adapter's source";

/// The kit ABI the sandbox supports is 1 today, so "current - 2" would underflow; 0 is a value
/// its load check refuses. When the kit moves on, this test follows the real `KIT_ABI`.
const OLD_ABI: u32 = nullrouter_adapter_kit::KIT_ABI.saturating_sub(2);

fn module(abi: u32) -> Vec<u8> {
    let b = abi.to_le_bytes().map(|b| format!("\\{b:02x}")).concat();
    wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
            (@custom "nr.abi" "{b}"))"#
    ))
    .unwrap()
}

struct Fixture {
    home: tempfile::TempDir,
    store: Store,
    alerts: Arc<AlertLog>,
    loader: Loader,
    name: HarnessName,
    id: VersionId,
    fp: String,
    old_hash: String,
}

fn approved_old() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let name = HarnessName::new("acme").unwrap();
    let files =
        [("adapter.toml".to_owned(), MANIFEST.as_bytes().to_vec()), ("src/lib.rs".to_owned(), LIB.as_bytes().to_vec())];
    let fp = fingerprint::of_files(&files);
    let id = VersionId::new(&semver::Version::new(1, 0, 0), &fp);
    for (path, bytes) in &files {
        store.write_version_file(&name, &id, &format!("source/{path}"), bytes).unwrap();
    }
    let wasm = module(OLD_ABI);
    store.write_version_file(&name, &id, "module.wasm", &wasm).unwrap();
    store.write_version_file(&name, &id, "build.json", b"{\"kit_abi\":0}").unwrap();
    let mut index = store.load_index().unwrap();
    let entry = VersionEntry {
        id: id.clone(),
        semver: "1.0.0".into(),
        state: VersionState::Queued,
        source_fp: fp.clone(),
        wasm_hash: Some(wasm_hash(&wasm)),
        kit_abi: Some(OLD_ABI),
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
    Fixture { home, store, alerts, loader, name, id, fp: fp.to_string(), old_hash: wasm_hash(&wasm) }
}

/// A builder script. `print` is the result line it ends with; with `output`, it first copies
/// the given module and `build.json` into the job's `out_dir`.
fn stub(dir: &Path, print: &str, output: Option<(&[u8], &str)>) -> String {
    let mut body = String::from("job=$(cat)\n");
    if let Some((wasm, build_json)) = output {
        fs::write(dir.join("new.wasm"), wasm).unwrap();
        fs::write(dir.join("new.build.json"), build_json).unwrap();
        body.push_str("out=$(printf '%s' \"$job\" | sed 's/.*\"out_dir\":\"\\([^\"]*\\)\".*/\\1/')\n");
        body.push_str(&format!("cp '{d}/new.wasm' \"$out/module.wasm\"\n", d = dir.display()));
        body.push_str(&format!("cp '{d}/new.build.json' \"$out/build.json\"\n", d = dir.display()));
    }
    body.push_str(&format!("cat <<'EOF'\n{print}\nEOF\n"));
    let path = dir.join("builder.sh");
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_str().unwrap().to_owned()
}

fn opts(builder: &str) -> InstallOptions<'_> {
    InstallOptions {
        styles: &[],
        builder: Some(builder),
        origin: Origin::Local("kit-upgrade".into()),
        build_timeout: Duration::from_secs(30),
    }
}

fn entry(f: &Fixture) -> VersionEntry {
    f.store.load_index().unwrap().version(&f.name, &f.id).unwrap().clone()
}

#[tokio::test]
async fn an_unsupported_abi_is_rebuilt_with_no_state_change_and_no_review() {
    let f = approved_old();
    let new_wasm = module(nullrouter_adapter_kit::KIT_ABI);
    let new_hash = wasm_hash(&new_wasm);
    assert_ne!(new_hash, f.old_hash);
    let build_json = format!(r#"{{"source_fp":"{}","wasm_hash":"{new_hash}","kit_abi":1}}"#, f.fp);
    let line = format!(
        r#"{{"ok":true,"source_fp":"{}","wasm_hash":"{new_hash}","kit_abi":{},"toolchain":"t"}}"#,
        f.fp,
        nullrouter_adapter_kit::KIT_ABI
    );
    let tools = tempfile::tempdir().unwrap();
    let builder = stub(tools.path(), &line, Some((&new_wasm, &build_json)));

    // At each reload the handle is read; the first reload follows the flagging.
    let reloads = AtomicUsize::new(0);
    let seen_first = std::sync::Mutex::new(None);
    let reload = || {
        if reloads.fetch_add(1, Ordering::SeqCst) == 0 {
            let h = f.loader.handle(&f.name);
            *seen_first.lock().unwrap() = Some((h.module.is_some(), h.reason));
        }
    };
    let done = startup_rebuilds(f.home.path(), &opts(&builder), &reload).await.unwrap();
    assert_eq!(done.len(), 1);
    // While flagged it did not load, and records show not_run{rebuilding}.
    assert_eq!(*seen_first.lock().unwrap(), Some((false, NotRunReason::Rebuilding)));

    let e = entry(&f);
    assert_eq!(e.state, VersionState::Approved);
    assert!(!e.rebuilding && !e.rebuild_failed);
    assert_eq!(e.wasm_hash.as_deref(), Some(new_hash.as_str()));
    assert_eq!(e.kit_abi, Some(nullrouter_adapter_kit::KIT_ABI));
    assert_eq!(e.source_fp.to_string(), f.fp);
    let index = f.store.load_index().unwrap();
    assert_eq!(index.harness(&f.name).unwrap().active.as_ref(), Some(&f.id));
    let dir = f.store.version_dir(&f.name, &f.id);
    assert_eq!(fs::read(dir.join("module.wasm")).unwrap(), new_wasm);
    assert_eq!(fs::read_to_string(dir.join("build.json")).unwrap(), build_json);
    // No review was enqueued: no report, no decision, no alert.
    assert!(!dir.join("report.json").exists() && !dir.join("decision.json").exists());
    assert!(f.alerts.list().unwrap().is_empty());
    // It serves.
    assert!(f.loader.handle(&f.name).module.is_some());
    // A second start finds nothing to do.
    assert!(startup_rebuilds(f.home.path(), &opts(&builder), &|| {}).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_rebuild_is_flagged_alerted_and_runs_as_a_plain_client() {
    let f = approved_old();
    let tools = tempfile::tempdir().unwrap();
    let builder = stub(tools.path(), r#"{"ok":false,"error":"compile","detail":"E0308"}"#, None);
    let done = startup_rebuilds(f.home.path(), &opts(&builder), &|| {}).await.unwrap();
    assert!(done.is_empty());

    let e = entry(&f);
    assert!(e.rebuild_failed && !e.rebuilding);
    assert_eq!(e.state, VersionState::Approved);
    assert_eq!(e.wasm_hash.as_deref(), Some(f.old_hash.as_str()));
    let alerts = f.alerts.list().unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, AlertKind::RebuildFailed);
    assert!(alerts[0].detail.contains("compile"), "{}", alerts[0].detail);
    let h = f.loader.handle(&f.name);
    assert!(h.module.is_none());
    assert_eq!(h.reason, NotRunReason::RebuildFailed);
    // Only the rebuild_failed alert: the load gate was never reached.
    assert_eq!(f.alerts.list().unwrap().len(), 1);
}

#[tokio::test]
async fn a_rebuild_for_other_source_is_refused() {
    let f = approved_old();
    let tools = tempfile::tempdir().unwrap();
    let line = r#"{"ok":true,"source_fp":"sha256:other","wasm_hash":"sha256:x","kit_abi":1,"toolchain":"t"}"#;
    let builder = stub(tools.path(), line, None);
    assert!(startup_rebuilds(f.home.path(), &opts(&builder), &|| {}).await.unwrap().is_empty());
    let e = entry(&f);
    assert!(e.rebuild_failed);
    assert_eq!(e.wasm_hash.as_deref(), Some(f.old_hash.as_str()));
    assert_eq!(f.alerts.list().unwrap()[0].kind, AlertKind::RebuildFailed);
}
