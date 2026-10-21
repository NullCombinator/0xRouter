//! The sandbox against WAT fixtures: what the gate refuses and what it lets through.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use nullrouter_sandbox::{
    CallError, Entry, LoadError, LoadedModule, ModuleFlags, Redactor, SandboxEngine, call, load, wasm_hash,
};

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

// ---- calls -------------------------------------------------------------------------------

const DEADLINE: Duration = Duration::from_millis(20);

fn plain() -> Redactor {
    Arc::new(|s: &str| s.to_owned())
}

/// A module whose `zr_on_request` has the given body, with the common exports and a 1-page memory.
fn with_request(body: &str) -> Vec<u8> {
    let on_request = format!(r#"(func (export "zr_on_request") (param $p i32) (param $l i32) (result i64) {body})"#);
    module(&[r#"(import "nr" "log" (func $log (param i32 i32)))"#, MEMORY, ALLOC, &on_request, ABI_1])
}

fn loaded(engine: &SandboxEngine, wasm: &[u8]) -> LoadedModule {
    load(engine, wasm, &wasm_hash(wasm), ModuleFlags::default()).expect("loads")
}

async fn run(wasm: &[u8], input: &[u8]) -> Result<Option<Vec<u8>>, CallError> {
    let engine = SandboxEngine::new(4).unwrap();
    let m = loaded(&engine, wasm);
    call(&engine, &m, Entry::Request, input, DEADLINE, plain()).await
}

#[tokio::test]
async fn a_valid_modules_output_round_trips() {
    // Returns the input pointer and length as the output.
    let echo = with_request("local.get $p i64.extend_i32_u i64.const 32 i64.shl local.get $l i64.extend_i32_u i64.or");
    let input = br#"{"ctx":{},"parts":[]}"#;
    assert_eq!(run(&echo, input).await.unwrap().as_deref(), Some(&input[..]));
}

#[tokio::test]
async fn a_zero_result_means_no_edits() {
    let none = with_request("i64.const 0");
    assert_eq!(run(&none, b"{}").await.unwrap(), None);
}

#[tokio::test]
async fn an_infinite_loop_returns_deadline_without_blocking_other_tasks() {
    let spin = with_request("(loop $l (br $l)) i64.const 0");
    let ticks = Arc::new(AtomicU32::new(0));
    let counter = ticks.clone();
    // On this single-threaded runtime the timer can only run if the guest yields.
    let timer = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1)).await;
            counter.fetch_add(1, Ordering::Relaxed);
        }
    });
    let started = Instant::now();
    let out = run(&spin, b"{}").await;
    let took = started.elapsed();
    timer.abort();

    assert_eq!(out, Err(CallError::Deadline));
    assert!(took >= DEADLINE, "stopped early: {took:?}");
    // The contract is the deadline plus 5 ms. A shared CI runner can be slower than that for one
    // scheduling hiccup, so the test allows more and the bench holds the tight figure.
    assert!(took < DEADLINE + Duration::from_millis(100), "stopped late: {took:?}");
    // A guest that never yielded would leave the timer with none: it is not polled while the
    // guest holds this thread. How many ticks fit in 20 ms depends on how fast the runner wakes
    // the 1 ms epoch thread, so the test asks only for more than one.
    let seen = ticks.load(Ordering::Relaxed);
    assert!(seen >= 2, "the timer task starved: {seen} ticks");
}

#[tokio::test]
async fn memory_grown_past_the_ceiling_returns_memory() {
    // 2000 pages is about 125 MB, over the 64 MiB ceiling.
    let big = with_request("i32.const 2000 memory.grow drop i64.const 0");
    assert_eq!(run(&big, b"{}").await, Err(CallError::Memory));
    // Growth within it is fine.
    let small = with_request("i32.const 100 memory.grow drop i64.const 0");
    assert_eq!(run(&small, b"{}").await, Ok(None));
}

#[tokio::test]
async fn an_engine_built_with_a_smaller_limit_refuses_growth_the_default_allows() {
    // 20 pages is 1.3 MB: over a 1 MiB limit, well within the default 64 MiB.
    let grow = with_request("i32.const 20 memory.grow drop i64.const 0");
    let small = SandboxEngine::with_memory_limit(4, 1 << 20).unwrap();
    assert_eq!(small.memory_limit(), 1 << 20);
    let m = loaded(&small, &grow);
    let out = call(&small, &m, Entry::Request, b"{}", DEADLINE, plain()).await;
    assert_eq!(out, Err(CallError::Memory));
    assert_eq!(run(&grow, b"{}").await, Ok(None));
}

#[tokio::test]
async fn a_trap_returns_trap() {
    let trap = with_request("unreachable");
    let err = run(&trap, b"{}").await.unwrap_err();
    assert!(matches!(err, CallError::Trap(_)), "{err}");
    assert_eq!(err.code(), "trap");
}

#[tokio::test]
async fn an_output_outside_the_modules_memory_is_invalid() {
    let outside = with_request("i64.const 0xFFFFFFF000000064");
    assert!(matches!(run(&outside, b"{}").await, Err(CallError::InvalidOutput(_))));
    // A length over the 16 MiB limit is refused before it is read.
    let huge = with_request("i64.const 0x0000040000000000 i64.const 0x02000000 i64.or");
    assert!(matches!(run(&huge, b"{}").await, Err(CallError::InvalidOutput(_))));
}

#[tokio::test]
async fn a_module_that_logs_too_much_or_out_of_bounds_still_completes() {
    // Twenty lines of 1000 bytes, then one pointing past the end of memory.
    let calls = "i32.const 0 i32.const 1000 call $log ".repeat(20);
    let chatty = with_request(&format!("{calls} i32.const -1 i32.const 10 call $log i64.const 0"));
    assert_eq!(run(&chatty, b"{}").await, Ok(None));
}

#[tokio::test]
async fn no_state_survives_from_one_call_to_the_next() {
    // Adds one to a global and returns it as a single digit. A reused instance would say 2.
    let counter = module(&[
        MEMORY,
        ALLOC,
        "(global $n (mut i32) (i32.const 0))",
        r#"(func (export "zr_on_request") (param i32 i32) (result i64)
            global.get $n i32.const 1 i32.add global.set $n
            i32.const 3000 global.get $n i32.const 48 i32.add i32.store8
            i64.const 3000 i64.const 32 i64.shl i64.const 1 i64.or)"#,
        ABI_1,
    ]);
    let engine = SandboxEngine::new(4).unwrap();
    let m = loaded(&engine, &counter);
    for _ in 0..3 {
        let out = call(&engine, &m, Entry::Request, b"{}", DEADLINE, plain()).await.unwrap();
        assert_eq!(out.as_deref(), Some(&b"1"[..]));
    }
}

#[tokio::test]
async fn an_input_over_the_limit_is_refused_before_it_is_written() {
    let none = with_request("i64.const 0");
    let engine = SandboxEngine::new(4).unwrap();
    let m = loaded(&engine, &none);
    let input = vec![b' '; (16 << 20) + 1];
    let out = call(&engine, &m, Entry::Request, &input, DEADLINE, plain()).await;
    assert_eq!(out, Err(CallError::InputTooLarge));
}
