//! The sandbox call cost (research R14): one fresh instance per call, plus the call (R3).
//!
//! - `sandbox/call_noop`: a module that answers no edits to every request, called with an input
//!   that carries no parts. This is the floor: instance creation and the call alone.
//! - `sandbox/call_claude_code`: the built Claude Code adapter with a 100 KB body as its part.
//!   It needs `crates/nullrouter-adapters/tests/fixtures/claude-code/module.wasm` (built by the
//!   operator, T080). Without the file the bench prints why and is skipped.
//!
//! `cargo bench -p nullrouter-sandbox --bench sandbox -- --save-baseline slice-004`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_adapter_kit::{Capabilities, Context, Direction, Part, Path};
use nullrouter_sandbox::{Entry, ModuleFlags, Redactor, SandboxEngine, call, load, wasm_hash};
use serde_json::json;

/// Longest a single call may run. The benches never reach it.
const DEADLINE: Duration = Duration::from_secs(5);

/// Answers no edits: `zr_on_request` returns 0, and the module has the ABI section the gate needs.
const NOOP: &str = r#"(module
    (import "nr" "abi_version" (func (result i32)))
    (memory (export "memory") 1)
    (func (export "zr_alloc") (param i32) (result i32) i32.const 1024)
    (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
    (@custom "nr.abi" "\01\00\00\00"))"#;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a Tokio runtime")
}

fn redactor() -> Redactor {
    Arc::new(|line: &str| line.to_owned())
}

/// The call input the core sends: the attempt's context and the parts the selectors matched.
fn input(parts: Vec<Part>) -> Vec<u8> {
    let ctx = Context {
        direction: Direction::Request,
        provider: "anthropic".to_owned(),
        target_style: "anthropic-messages".to_owned(),
        same_style: true,
        model: "claude-sonnet".to_owned(),
        model_type: "text".to_owned(),
        capabilities: Capabilities::default(),
        stream: false,
        attempt: 1,
    };
    serde_json::to_vec(&json!({"ctx": ctx, "parts": parts})).expect("the input serialises")
}

fn call_noop(c: &mut Criterion) {
    let rt = runtime();
    let engine = SandboxEngine::new(4).expect("the sandbox engine starts");
    let wasm = wat::parse_str(NOOP).expect("the fixture is valid WAT");
    let module = load(&engine, &wasm, &wasm_hash(&wasm), ModuleFlags::default()).expect("the fixture loads");
    let input = input(Vec::new());
    let redact = redactor();
    c.bench_function("sandbox/call_noop", |b| {
        b.iter(|| rt.block_on(call(&engine, &module, Entry::Request, black_box(&input), DEADLINE, redact.clone())));
    });
}

fn call_claude_code(c: &mut Criterion) {
    let path = format!("{}/../nullrouter-adapters/tests/fixtures/claude-code/module.wasm", env!("CARGO_MANIFEST_DIR"));
    let Ok(wasm) = std::fs::read(&path) else {
        eprintln!("sandbox/call_claude_code skipped: no built module at {path} (T080)");
        return;
    };
    let engine = match SandboxEngine::new(4) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("sandbox/call_claude_code skipped: {e}");
            return;
        }
    };
    let module = match load(&engine, &wasm, &wasm_hash(&wasm), ModuleFlags::default()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("sandbox/call_claude_code skipped: the module does not load: {e}");
            return;
        }
    };
    let rt = runtime();
    let body = json!({
        "model": "claude-sonnet",
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": "x".repeat(100_000)}]
    });
    let input = input(vec![Part { path: Path::root(), value: body }]);
    let redact = redactor();
    c.bench_function("sandbox/call_claude_code", |b| {
        b.iter(|| rt.block_on(call(&engine, &module, Entry::Request, black_box(&input), DEADLINE, redact.clone())));
    });
}

criterion_group!(benches, call_noop, call_claude_code);
criterion_main!(benches);
