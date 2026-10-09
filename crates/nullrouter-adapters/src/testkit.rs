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

/// Where a WAT module keeps the answer it returns. Inputs are copied in at 4096, well below it.
const OUT_AT: i64 = 32768;

/// How a WAT test adapter behaves when the host calls it. Each variant is one hostile or guard case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Behaviour {
    /// Answers `0` (no edits) to every request.
    NoEdits,
    /// Answers the given edits JSON (the `{"edits": [...]}` format) from a data segment.
    Edits(String),
    /// Spins forever in `zr_on_request`; the sandbox must stop it at the deadline.
    Loop,
    /// Grows linear memory to 2048 pages (128 MiB), past the 64 MiB cap, and ignores the answer.
    GrowMemory,
    /// Executes `unreachable` in `zr_on_request`.
    Trap,
    /// Imports `module.name` (a function the host never grants) and otherwise answers no edits.
    ExtraImport(&'static str, &'static str),
    /// Leaves out the named export (`memory`, `zr_alloc` or `zr_on_request`) and otherwise
    /// answers no edits.
    MissingExport(&'static str),
    /// Omits the `nr.abi` custom section and otherwise answers no edits.
    NoAbiSection,
}

/// Returns the binary module for a small adapter that behaves as `b` says, ready for
/// `install_fixture`. The module is assembled from WAT text with `wat::parse_str`.
pub fn wat_adapter(b: Behaviour) -> Vec<u8> {
    let mut body = String::from("i64.const 0");
    let mut data = String::new();
    let mut imports = String::new();
    let mut missing = "";
    let mut abi = r#"(@custom "nr.abi" "\01\00\00\00")"#;
    match b {
        Behaviour::NoEdits => {}
        Behaviour::Edits(json) => {
            body = format!("i64.const {}", (OUT_AT << 32) | json.len() as i64);
            data = format!(r#"(data (i32.const {OUT_AT}) "{}")"#, wat_escape(&json));
        }
        Behaviour::Loop => body = "(loop $l (br $l))\n i64.const 0".into(),
        Behaviour::GrowMemory => body = "(drop (memory.grow (i32.const 2048)))\n i64.const 0".into(),
        Behaviour::Trap => body = "unreachable".into(),
        Behaviour::ExtraImport(module, name) => {
            imports = format!(r#"(import "{module}" "{name}" (func (param i32 i32 i32 i32) (result i32)))"#);
        }
        Behaviour::MissingExport(name) => missing = name,
        Behaviour::NoAbiSection => abi = "",
    }
    let export = |name: &str| if missing == name { String::new() } else { format!(r#"(export "{name}")"#) };
    let text = format!(
        r#"(module
            {imports}
            (memory {memory} 1)
            {data}
            (func {alloc} (param i32) (result i32) i32.const 4096)
            (func {on_request} (param i32 i32) (result i64) {body})
            {abi})"#,
        memory = export("memory"),
        alloc = export("zr_alloc"),
        on_request = export("zr_on_request"),
    );
    wat::parse_str(&text).expect("a well-formed test module")
}

/// Escapes `s` for a WAT string literal: quotes and backslashes as hex, other non-printables too.
fn wat_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'"' => "\\22".to_owned(),
            b'\\' => "\\5c".to_owned(),
            0x20..=0x7e => (b as char).to_string(),
            _ => format!("\\{b:02x}"),
        })
        .collect()
}
