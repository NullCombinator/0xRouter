//! Helpers for tests of the store-backed path (T047). Only built with the `testkit` feature.

use std::path::Path;

use nullrouter_sandbox::wasm_hash;

use crate::HarnessName;
use crate::fingerprint;
use crate::store::{Origin, Store, VersionEntry, VersionId, VersionState};

/// Writes an `approved` version of `harness` into the store under `home` and makes it the
/// active one, with the hashes its files really have. `manifest` is `adapter.toml`; `wasm` is
/// the module.
pub fn install_fixture(home: &Path, harness: &str, manifest: &str, wasm: &[u8]) -> VersionId {
    let store = Store::open(home).expect("the store opens");
    let name = HarnessName::new(harness).expect("a valid harness name");
    let files =
        [("adapter.toml".to_owned(), manifest.as_bytes().to_vec()), ("src/lib.rs".to_owned(), b"// fixture".to_vec())];
    let fp = fingerprint::of_files(&files);
    let semver = semver::Version::new(1, 0, 0);
    let id = VersionId::new(&semver, &fp);
    for (path, bytes) in &files {
        store.write_version_file(&name, &id, &format!("source/{path}"), bytes).expect("source written");
    }
    store.write_version_file(&name, &id, "module.wasm", wasm).expect("module written");

    let mut index = store.load_index().expect("the index reads");
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
        origin: Origin::Local("testkit".into()),
    };
    index.submit(&name, entry).expect("a new version");
    for to in [VersionState::Building, VersionState::InReview, VersionState::Reported] {
        index.transition(&name, &id, to, "").expect("an edge of the state machine");
    }
    index.approve(&name, &id).expect("approval");
    store.save_index(&index).expect("the index saves");
    id
}
