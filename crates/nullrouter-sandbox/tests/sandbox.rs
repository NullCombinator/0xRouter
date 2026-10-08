//! The sandbox against WAT fixtures: what the gate refuses and what it lets through.

use nullrouter_sandbox::{LoadError, ModuleFlags, SandboxEngine, load, wasm_hash};

const ABI_1: &str = r#"(@custom "nr.abi" "\01\00\00\00")"#;
const MEMORY: &str = r#"(memory (export "memory") 1)"#;
const ALLOC: &str = r#"(func (export "zr_alloc") (param i32) (result i32) i32.const 1024)"#;
const ON_REQUEST: &str = r#"(func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)"#;

fn module(parts: &[&str]) -> Vec<u8> {
    wat::parse_str(format!("(module {})", parts.join("\n"))).expect("the fixture is valid WAT")
}

/// A module with everything the gate requires.
fn valid() -> Vec<u8> {
    module(&[r#"(import "nr" "abi_version" (func (result i32)))"#, MEMORY, ALLOC, ON_REQUEST, ABI_1])
}

fn try_load(wasm: &[u8], flags: ModuleFlags) -> Result<(), LoadError> {
    let engine = SandboxEngine::new(4).expect("engine");
    load(&engine, wasm, &wasm_hash(wasm), flags).map(|_| ())
}

fn refused(wasm: &[u8]) -> LoadError {
    try_load(wasm, ModuleFlags::default()).expect_err("the module is refused")
}

#[tokio::test]
async fn a_valid_module_loads() {
    let wasm = valid();
    let engine = SandboxEngine::new(4).unwrap();
    let loaded = load(&engine, &wasm, &wasm_hash(&wasm), ModuleFlags::default()).expect("loads");
    assert_eq!(loaded.abi(), 1);
}

#[tokio::test]
async fn the_hash_form_is_sha256_and_hex() {
    let h = wasm_hash(b"x");
    assert_eq!(h, "sha256:2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881");
}

#[tokio::test]
async fn a_module_that_is_not_the_recorded_one_is_refused() {
    let wasm = valid();
    let engine = SandboxEngine::new(4).unwrap();
    let wrong = wasm_hash(b"something else");
    let err = load(&engine, &wasm, &wrong, ModuleFlags::default()).unwrap_err();
    assert!(matches!(err, LoadError::HashMismatch { .. }), "{err}");
}

#[tokio::test]
async fn wat_text_is_refused_even_when_the_hash_matches() {
    let text = b"(module)".to_vec();
    assert!(matches!(refused(&text), LoadError::NotBinary));
}

#[tokio::test]
async fn any_import_beyond_the_two_host_functions_is_refused_and_named() {
    let cases = [
        (
            "wasi_snapshot_preview1",
            "fd_write",
            r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#,
        ),
        ("env", "socket", r#"(import "env" "socket" (func (param i32) (result i32)))"#),
        ("nr", "exec", r#"(import "nr" "exec" (func))"#),
        ("other", "abi_version", r#"(import "other" "abi_version" (func (result i32)))"#),
    ];
    for (m, n, import) in cases {
        let wasm = module(&[import, MEMORY, ALLOC, ON_REQUEST, ABI_1]);
        match refused(&wasm) {
            LoadError::ForbiddenImport { module, name } => assert_eq!((module.as_str(), name.as_str()), (m, n)),
            other => panic!("{m}.{n}: {other}"),
        }
    }
}

#[tokio::test]
async fn a_host_function_with_the_wrong_signature_does_not_link() {
    let wasm = module(&[r#"(import "nr" "abi_version" (func (param i32)))"#, MEMORY, ALLOC, ON_REQUEST, ABI_1]);
    assert!(matches!(refused(&wasm), LoadError::Link(_)));
}

#[tokio::test]
async fn a_missing_required_export_is_refused() {
    let no_memory = module(&[ALLOC, ON_REQUEST, ABI_1]);
    assert!(matches!(refused(&no_memory), LoadError::MissingExport("memory")));
    let no_alloc = module(&[MEMORY, ON_REQUEST, ABI_1]);
    assert!(matches!(refused(&no_alloc), LoadError::MissingExport("zr_alloc")));
    let no_request = module(&[MEMORY, ALLOC, ABI_1]);
    assert!(matches!(refused(&no_request), LoadError::MissingExport("zr_on_request")));
}

#[tokio::test]
async fn an_export_of_the_wrong_type_is_refused() {
    let bad_alloc =
        module(&[MEMORY, r#"(func (export "zr_alloc") (param i32) (result i64) i64.const 0)"#, ON_REQUEST, ABI_1]);
    assert!(matches!(refused(&bad_alloc), LoadError::BadExport("zr_alloc")));
    let memory_as_func = module(&[r#"(func (export "memory"))"#, ALLOC, ON_REQUEST, ABI_1]);
    assert!(matches!(refused(&memory_as_func), LoadError::BadExport("memory")));
}

#[tokio::test]
async fn the_response_and_event_exports_are_required_only_when_the_manifest_asks() {
    let wasm = valid();
    let response = ModuleFlags { response: true, events: false };
    let events = ModuleFlags { response: false, events: true };
    assert!(matches!(try_load(&wasm, response), Err(LoadError::MissingExport("zr_on_response"))));
    assert!(matches!(try_load(&wasm, events), Err(LoadError::MissingExport("zr_on_event"))));
    let full = module(&[
        MEMORY,
        ALLOC,
        ON_REQUEST,
        r#"(func (export "zr_on_response") (param i32 i32) (result i64) i64.const 0)"#,
        r#"(func (export "zr_on_event") (param i32 i32) (result i64) i64.const 0)"#,
        ABI_1,
    ]);
    try_load(&full, ModuleFlags { response: true, events: true }).expect("loads with both");
}

#[tokio::test]
async fn a_module_without_the_abi_section_or_with_an_unsupported_one_is_refused() {
    let none = module(&[MEMORY, ALLOC, ON_REQUEST]);
    assert!(matches!(refused(&none), LoadError::AbiMissing));
    for abi in [r#"(@custom "nr.abi" "\09\00\00\00")"#, r#"(@custom "nr.abi" "\00\00\00\00")"#] {
        let wasm = module(&[MEMORY, ALLOC, ON_REQUEST, abi]);
        assert!(matches!(refused(&wasm), LoadError::AbiUnsupported(_)), "{abi}");
    }
    let short = module(&[MEMORY, ALLOC, ON_REQUEST, r#"(@custom "nr.abi" "\01")"#]);
    assert!(matches!(refused(&short), LoadError::AbiMalformed));
}
