//! The Claude Code adapter against its 9router oracle (spec 004, T080).
//!
//! The adapter runs as production runs it: the module built from
//! `adapters/community/claude-code` by the builder (`tools/build-adapter-fixtures.sh`), installed
//! with the testkit, loaded by the loader and called through the sandbox, with the guardrail on.
//! Its output must equal 9router's `normalizeClaudePassthrough` + `dedupeTools` on every
//! fixture under `tests/fixtures/9router/adapters/claude-code`, outside the rows listed for
//! `harness = "claude-code"` in `tests/parity/deviations.toml`.

use std::path::PathBuf;
use std::sync::Arc;

use nullrouter_adapter_kit::{Capabilities, Context, Direction};
use nullrouter_adapters::HarnessName;
use nullrouter_adapters::fingerprint;
use nullrouter_adapters::loader::Loader;
use nullrouter_adapters::record::{AdapterOutcome, AdapterRun};
use nullrouter_adapters::runner::AdapterRunner;
use nullrouter_adapters::testkit::install_fixture;
use nullrouter_registry::validate::validate_style;
use nullrouter_sandbox::wasm_hash;
use nullrouter_wire::codec::Style;
use serde::Deserialize;
use serde_json::Value;

const HARNESS: &str = "claude-code";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-code")
}

fn source_dir() -> PathBuf {
    root().join("adapters/community/claude-code")
}

fn read(path: PathBuf) -> Vec<u8> {
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}. Build the claude-code fixture: nullrouter-builder setup, then tools/build-adapter-fixtures.sh",
            path.display()
        )
    })
}

fn json(path: PathBuf) -> Value {
    serde_json::from_slice(&read(path)).unwrap()
}

/// The fixture's `build.json`.
fn build_record() -> Value {
    json(fixture_dir().join("build.json"))
}

fn anthropic_messages() -> Arc<Style> {
    let path = root().join("styles/bundled/anthropic-messages.toml");
    let src = std::fs::read_to_string(&path).unwrap();
    let parsed = validate_style(&src, &path.display().to_string()).unwrap_or_else(|e| panic!("{e:#?}"));
    Arc::new(Style::compile(&parsed).unwrap())
}

/// The built adapter, installed and loaded from a fresh store.
struct Adapter {
    _home: tempfile::TempDir,
    runner: AdapterRunner,
}

impl Adapter {
    fn load() -> Adapter {
        let home = tempfile::tempdir().unwrap();
        let manifest = std::fs::read_to_string(source_dir().join("adapter.toml")).unwrap();
        install_fixture(home.path(), HARNESS, &manifest, &read(fixture_dir().join("module.wasm")));
        let loader = Loader::open(home.path(), Arc::new(|s: &str| s.to_owned())).unwrap();
        let handle = loader.handle(&HarnessName::new(HARNESS).unwrap());
        assert!(handle.module.is_some(), "the module does not load: {:?}", handle.reason);
        Adapter { _home: home, runner: AdapterRunner::Wasm(handle).with_client(anthropic_messages()) }
    }

    /// The body after the adapter, and the run. The guardrail passing is part of every run:
    /// a blocked run would leave the outcome `Blocked`.
    async fn run(&self, ctx: &Context, body: &Value) -> (Value, AdapterRun) {
        let out = self.runner.run_request(ctx, body).await;
        assert_eq!(out.run.outcome, AdapterOutcome::Ran, "{:?}", out.run);
        assert!(out.run.guardrail.is_none(), "{:?}", out.run.guardrail);
        (out.body.into_owned(), out.run)
    }
}

/// 9router's `claude` provider is 0router's `anthropic`, called in its own style.
fn ctx(provider: &str, target_style: &str, model: &str, stream: bool) -> Context {
    Context {
        direction: Direction::Request,
        provider: provider.into(),
        target_style: target_style.into(),
        same_style: target_style == "anthropic-messages",
        model: model.into(),
        model_type: "text".into(),
        capabilities: Capabilities::default(),
        stream,
        attempt: 1,
    }
}

struct Case {
    name: String,
    ctx: Value,
    input: Value,
    want: Value,
}

fn cases() -> Vec<Case> {
    let dir = root().join("tests/fixtures/9router/adapters/claude-code");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok()?.strip_suffix(".in.json").map(str::to_owned))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no fixtures under {}", dir.display());
    let data = |name: &str, kind: &str| json(dir.join(format!("{name}.{kind}.json")))["data"].take();
    names
        .into_iter()
        .map(|name| Case { ctx: data(&name, "ctx"), input: data(&name, "in"), want: data(&name, "out"), name })
        .collect()
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    deviation: Vec<Row>,
}

#[derive(Debug, Deserialize)]
struct Row {
    harness: Option<String>,
    fixture: String,
    field: String,
}

/// This harness's rows of `tests/parity/deviations.toml`.
fn deviations() -> Vec<Row> {
    let src = std::fs::read_to_string(root().join("tests/parity/deviations.toml")).unwrap();
    let file: File = toml::from_str(&src).unwrap();
    file.deviation.into_iter().filter(|r| r.harness.as_deref() == Some(HARNESS)).collect()
}

/// Takes the value at a dotted path (array indices as numbers) out of `v`, leaving `null`.
fn take(v: &mut Value, field: &str) -> Value {
    let pointer = format!("/{}", field.replace('.', "/"));
    v.pointer_mut(&pointer).map(Value::take).unwrap_or(Value::Null)
}

/// The companion check: the checked-in module was built from the adapter source as it is now.
#[test]
fn the_fixture_is_built_from_the_current_source() {
    let record = build_record();
    let fp = fingerprint::of_dir(&source_dir()).unwrap();
    assert_eq!(
        record["source_fp"].as_str(),
        Some(fp.as_str()),
        "adapters/community/claude-code changed since the fixture was built: rerun tools/build-adapter-fixtures.sh"
    );
    let wasm = read(fixture_dir().join("module.wasm"));
    assert_eq!(record["wasm_hash"].as_str(), Some(wasm_hash(&wasm).as_str()), "module.wasm and build.json disagree");
}

#[tokio::test]
async fn every_fixture_matches_9router_outside_the_listed_deviations() {
    let adapter = Adapter::load();
    let rows = deviations();
    let cases = cases();
    let mut failures = Vec::new();
    for case in &cases {
        assert_eq!(case.ctx["provider"], "anthropic", "{}: an Anthropic context", case.name);
        let model = case.ctx["model"].as_str().unwrap();
        let stream = case.input["stream"].as_bool().unwrap_or(false);
        let (mut got, _) = adapter.run(&ctx("anthropic", "anthropic-messages", model, stream), &case.input).await;
        let mut want = case.want.clone();
        for row in rows.iter().filter(|r| r.fixture == case.name) {
            if take(&mut got, &row.field) == take(&mut want, &row.field) {
                failures.push(format!("{}: listed deviation `{}` no longer differs", case.name, row.field));
            }
        }
        if got != want {
            failures.push(format!("{}:\n  got  {got}\n  want {want}", case.name));
        }
    }
    for row in &rows {
        if !cases.iter().any(|c| c.name == row.fixture) {
            failures.push(format!("deviation for missing fixture {}", row.fixture));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The placeholder case's deviation, asserted as 0router's own behaviour: the tool-use turn
/// keeps its blocks and gains nothing, and the foreign thinking block is removed.
#[tokio::test]
async fn deviation_no_thinking_placeholder_is_inserted() {
    let adapter = Adapter::load();
    let case = cases().into_iter().find(|c| c.name == "thinking-placeholder").unwrap();
    let model = case.ctx["model"].as_str().unwrap();
    let (got, run) = adapter.run(&ctx("anthropic", "anthropic-messages", model, true), &case.input).await;
    assert_eq!(got["messages"][1], case.input["messages"][1], "the first tool-use turn is left as is");
    let types: Vec<&str> =
        got["messages"][3]["content"].as_array().unwrap().iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["tool_use"]);
    let changes: Vec<String> = run.changes.iter().map(|c| format!("{} {:?}/{:?}", c.path, c.kind, c.reason)).collect();
    assert_eq!(changes, ["messages[3].content[0] Removed/ForeignBlock"]);
}

/// On any target that is not Anthropic in its own style, Anthropic's server-tool blocks are
/// removed, each recorded with `target_cannot_carry_block`.
#[tokio::test]
async fn server_tool_blocks_are_removed_for_a_non_anthropic_target() {
    let adapter = Adapter::load();
    let case = cases().into_iter().find(|c| c.name == "server-tool-use-srvtoolu").unwrap();
    let model = case.ctx["model"].as_str().unwrap();
    for (provider, style) in
        [("openrouter", "openai-chat"), ("deepseek", "anthropic-messages"), ("anthropic", "openai-chat")]
    {
        let (got, run) = adapter.run(&ctx(provider, style, model, true), &case.input).await;
        let mut want = case.input.clone();
        let content = want["messages"][1]["content"].as_array_mut().unwrap();
        content.retain(|b| !matches!(b["type"].as_str(), Some("server_tool_use" | "web_search_tool_result")));
        assert_eq!(got, want, "{provider}/{style}");
        let changes: Vec<String> =
            run.changes.iter().map(|c| format!("{} {:?}/{:?}", c.path, c.kind, c.reason)).collect();
        assert_eq!(
            changes,
            [
                "messages[1].content[1] Removed/TargetCannotCarryBlock",
                "messages[1].content[2] Removed/TargetCannotCarryBlock"
            ],
            "{provider}/{style}"
        );
    }
}
