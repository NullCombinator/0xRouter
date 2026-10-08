//! The request side of a third-party adapter, end to end through the sandbox (T046): small WAT
//! modules stand in for built adapters, so these run without the builder.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nullrouter_adapter_kit::{Capabilities, Context, Direction};
use nullrouter_adapters::apply::Rule;
use nullrouter_adapters::record::{AdapterDirection, AdapterOutcome, FailReason, GuardrailRule, NotRunReason};
use nullrouter_adapters::runner::{AdapterRunner, WasmHandle, WasmModule};
use nullrouter_adapters::selector::Selector;
use nullrouter_registry::validate::validate_style;
use nullrouter_sandbox::{ModuleFlags, SandboxEngine, load, wasm_hash};
use nullrouter_wire::codec::Style;
use serde_json::{Value, json};

const HARNESS: &str = "acme";
const VERSION: &str = "v1.0.0-abcd1234";
/// Where a module keeps the answer it returns. Inputs are copied in at 4096, well below it.
const OUT_AT: i64 = 32768;

fn ctx() -> Context {
    Context {
        direction: Direction::Request,
        provider: "groq".into(),
        target_style: "openai-chat".into(),
        same_style: true,
        model: "m".into(),
        model_type: "text".into(),
        capabilities: Capabilities::default(),
        stream: false,
        attempt: 1,
    }
}

fn chat() -> Arc<Style> {
    let path = format!("{}/../../styles/bundled/openai-chat.toml", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    let parsed = validate_style(&src, &path).unwrap_or_else(|e| panic!("openai-chat fails the style gate: {e:#?}"));
    Arc::new(Style::compile(&parsed).unwrap())
}

fn data_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'"' => "\\22".to_owned(),
            b'\\' => "\\5c".to_owned(),
            0x20..=0x7e => (b as char).to_string(),
            _ => format!("\\{b:02x}"),
        })
        .collect()
}

/// A module whose `zr_on_request` runs `body` (WAT instructions leaving an i64), with `data`
/// stored where an answer would be read from.
fn module(body: &str, data: &str) -> Vec<u8> {
    let data = data_escape(data);
    wat::parse_str(format!(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const {OUT_AT}) "{data}")
            (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "zr_on_request") (param i32 i32) (result i64) {body})
            (@custom "nr.abi" "\01\00\00\00"))"#
    ))
    .expect("the fixture is valid WAT")
}

/// A module that answers `output` on every request.
fn answering(output: &Value) -> Vec<u8> {
    let text = output.to_string();
    module(&format!("i64.const {}", (OUT_AT << 32) | text.len() as i64), &text)
}

fn runner(wasm: &[u8], selectors: &[&str]) -> AdapterRunner {
    let engine = Arc::new(SandboxEngine::new(4).expect("engine"));
    let loaded = load(&engine, wasm, &wasm_hash(wasm), ModuleFlags::default()).expect("the module loads");
    AdapterRunner::Wasm(WasmHandle::loaded(
        HARNESS,
        VERSION,
        WasmModule {
            sandbox: engine,
            module: Arc::new(loaded),
            request_selectors: selectors.iter().map(|s| Selector::parse(s).unwrap()).collect(),
            request_deadline: Duration::from_millis(20),
            redact: Arc::new(|s: &str| s.to_owned()),
        },
    ))
}

fn conversation() -> Value {
    json!({"model": "m", "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "checking", "reasoning_content": "thinking",
         "tool_calls": [{"id": "c1", "type": "function",
                         "function": {"name": "get_weather", "arguments": "{\"city\":\"Oslo\"}"}}]},
        {"role": "tool", "tool_call_id": "c1", "content": "rain"}
    ]})
}

fn remove_reasoning() -> Value {
    json!({"edits": [{"op": "remove", "path": "messages[1].reasoning_content",
                      "kind": "removed", "reason": "target_rejects_field"}]})
}

fn replace_tool_calls(calls: Value) -> Value {
    json!({"edits": [{"op": "replace", "path": "messages[1].tool_calls", "kind": "converted",
                      "reason": "format_conversion", "value": calls}]})
}

fn call(id: &str, name: &str, args: &str) -> Value {
    json!({"id": id, "type": "function", "function": {"name": name, "arguments": args}})
}

const REASONING: &[&str] = &["messages[*].reasoning_content"];
const TOOL_CALLS: &[&str] = &["messages[*].tool_calls"];

#[tokio::test]
async fn a_harness_with_no_approved_version_records_that_and_changes_nothing() {
    let r = AdapterRunner::Wasm(WasmHandle::absent(HARNESS));
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::NotRun { reason: NotRunReason::NoApprovedVersion });
    assert!(matches!(out.body, Cow::Borrowed(_)));
}

#[tokio::test]
async fn a_body_the_selectors_do_not_match_never_reaches_the_module() {
    // The module would trap if it were called.
    let r = runner(&module("unreachable", ""), &["messages[*].images"]).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::NotRun { reason: NotRunReason::NoSelectorMatch });
    assert!(matches!(out.body, Cow::Borrowed(_)));
}

#[tokio::test]
async fn a_removal_is_applied_to_a_copy_and_recorded_by_path_and_reason() {
    let r = runner(&answering(&remove_reasoning()), REASONING).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Ran, "{:?}", out.run);
    assert_eq!((out.run.harness.as_str(), out.run.version.as_str()), (HARNESS, VERSION));
    assert!(out.body.pointer("/messages/1/reasoning_content").is_none());
    assert_eq!(out.body.pointer("/messages/1/tool_calls"), body.pointer("/messages/1/tool_calls"));
    let paths: Vec<String> = out.run.changes.iter().map(|c| c.path.clone()).collect();
    assert_eq!(paths, ["messages[1].reasoning_content"]);
    // The caller's body is untouched.
    assert!(body.pointer("/messages/1/reasoning_content").is_some());
}

#[tokio::test]
async fn a_module_that_makes_no_edits_leaves_the_body_borrowed() {
    let r = runner(&module("i64.const 0", ""), REASONING).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Ran);
    assert!(matches!(out.body, Cow::Borrowed(_)));
    assert!(out.run.changes.is_empty());
}

#[tokio::test]
async fn a_trap_fails_the_run_and_the_original_goes_on() {
    let r = runner(&module("unreachable", ""), REASONING).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Failed { reason: FailReason::Trap });
    assert!(matches!(out.body, Cow::Borrowed(_)));
}

#[tokio::test]
async fn a_loop_is_stopped_at_the_deadline() {
    let r = runner(&module("(loop $l (br $l))\n i64.const 0", ""), REASONING).with_client(chat());
    let body = conversation();
    let started = Instant::now();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Failed { reason: FailReason::Deadline });
    assert!(matches!(out.body, Cow::Borrowed(_)));
    assert!(started.elapsed() < Duration::from_secs(2), "the loop ran for {:?}", started.elapsed());
}

#[tokio::test]
async fn output_that_is_not_json_is_an_invalid_output() {
    let text = "not json";
    let wasm = module(&format!("i64.const {}", (OUT_AT << 32) | text.len() as i64), text);
    let r = runner(&wasm, REASONING).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: Rule::NotJson } });
}

#[tokio::test]
async fn an_edit_outside_the_declared_selectors_is_refused() {
    let edit = json!({"edits": [{"op": "remove", "path": "messages[0].content",
                                 "kind": "removed", "reason": "target_rejects_field"}]});
    let r = runner(&answering(&edit), REASONING).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(
        out.run.outcome,
        AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: Rule::OutsideSelector } }
    );
    assert!(matches!(out.body, Cow::Borrowed(_)));
}

#[tokio::test]
async fn a_reason_code_the_kit_does_not_know_is_refused() {
    let edit = json!({"edits": [{"op": "remove", "path": "messages[1].reasoning_content",
                                 "kind": "removed", "reason": "because"}]});
    let r = runner(&answering(&edit), REASONING).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(
        out.run.outcome,
        AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: Rule::UnknownReason } }
    );
}

#[tokio::test]
async fn changing_a_tool_calls_arguments_is_blocked_and_the_original_goes_on() {
    let changed = replace_tool_calls(json!([call("c1", "get_weather", "{\"city\":\"Paris\"}")]));
    let r = runner(&answering(&changed), TOOL_CALLS).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Blocked, "{:?}", out.run);
    assert!(matches!(out.body, Cow::Borrowed(_)));
    assert!(out.run.changes.is_empty(), "nothing was applied, so nothing is recorded as changed");
    let event = out.run.guardrail.expect("a guardrail event");
    assert_eq!(event.rule, GuardrailRule::ToolCallChanged);
    assert_eq!(event.direction, AdapterDirection::Request);
    assert_eq!((event.adapter.harness.as_str(), event.adapter.version.as_str()), (HARNESS, VERSION));
    assert!(!event.paths.is_empty() && event.paths.iter().all(|p| !p.contains("Paris")), "{:?}", event.paths);
}

#[tokio::test]
async fn adding_a_tool_call_is_blocked() {
    let more = replace_tool_calls(json!([
        call("c1", "get_weather", "{\"city\":\"Oslo\"}"),
        call("c2", "rm", "{\"path\":\"/\"}")
    ]));
    let r = runner(&answering(&more), TOOL_CALLS).with_client(chat());
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(out.run.outcome, AdapterOutcome::Blocked, "{:?}", out.run);
    assert_eq!(out.run.guardrail.map(|g| g.rule), Some(GuardrailRule::ToolCallAdded));
}

#[tokio::test]
async fn with_no_client_style_an_edit_is_never_applied() {
    let r = runner(&answering(&remove_reasoning()), REASONING);
    let body = conversation();
    let out = r.run_request(&ctx(), &body).await;
    assert_eq!(
        out.run.outcome,
        AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: Rule::Undecodable } }
    );
    assert!(matches!(out.body, Cow::Borrowed(_)));
    assert!(out.run.changes.is_empty());
}
