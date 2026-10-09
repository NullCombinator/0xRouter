//! The no-content sentinel (spec 004, T086; FR-025, SC-009). Hermes, the `legit_removal`
//! hostile case and the Claude Code adapter run over bodies that hold a unique sentinel in
//! every value they remove or convert. The whole record store, the journal files, the alerts
//! file and the log capture are then searched: no sentinel may appear anywhere, and every change
//! the adapter made must be listed with its path, kind and reason.
//!
//! The Claude Code cases need the operator-built module (T080) and print a skip notice without it.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::{Server, reply_by_wire, server_custom};
use nullrouter_adapters::alerts::AlertLog;
use nullrouter_adapters::record::{AdapterOutcome, AdapterRun};
use nullrouter_adapters::store::Store;
use nullrouter_adapters::testkit::install_fixture;
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::{Query, RequestRecord};
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

/// Every test's tracing output, at TRACE, in one buffer (the subscriber is global).
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn logs() -> Captured {
    static LOGS: OnceLock<Captured> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = Captured::default();
        let sink = logs.clone();
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .try_init();
        logs
    })
    .clone()
}

/// A sentinel unique to the case, the value and the run.
fn sentinel(case: &str, n: usize) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("NRS-{case}-{n}-{nanos:x}")
}

/// A base64-looking image string that sniffs as PNG and carries `token` (letters and digits).
fn png_with(token: &str) -> String {
    let mut s = format!("iVBORw0KGgoAAAANSUhE{token}");
    while s.len() % 4 != 0 {
        s.push('A');
    }
    s
}

fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if let Ok(bytes) = std::fs::read(&p) {
            out.push((format!("file {}", p.display()), String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
}

/// Everything the run left behind that must hold no content.
fn places(s: &Server, log: &Captured) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for r in s.engine.records.query(&Query::default()) {
        out.push((format!("record {}", r.id), serde_json::to_string(&r).unwrap()));
    }
    s.engine.journal.flush_blocking();
    walk(&s.home().join("records"), &mut out);
    walk(&s.home().join("routing"), &mut out);
    if let Ok(text) = std::fs::read_to_string(s.home().join("adapters").join("alerts.toml")) {
        out.push(("alerts.toml".into(), text));
    }
    let alerts = AlertLog::open(&Store::open(s.home()).unwrap()).list();
    out.push(("alert log".into(), format!("{alerts:?}")));
    out.push(("log capture".into(), log.text()));
    out
}

fn assert_no_sentinel(case: &str, s: &Server, log: &Captured, sentinels: &[String]) {
    let all = places(s, log);
    assert!(all.iter().any(|(p, _)| p.starts_with("record ")), "{case}: no record was kept to search");
    let mut leaks = Vec::new();
    for (place, text) in &all {
        for sn in sentinels {
            if text.contains(sn.as_str()) {
                leaks.push(format!("{place} holds {sn}"));
            }
        }
    }
    assert!(leaks.is_empty(), "{case}: content reached what must hold none: {leaks:#?}");
}

/// Binds the server's key to `harness`.
fn bind(s: &Server, harness: &str) {
    let path = s.home().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    keys.set_adapter("laptop", Some(HarnessName::new(harness).unwrap())).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
}

async fn send(s: &Server, path: &str, anthropic: bool, body: &Value) -> RequestRecord {
    let mut req = reqwest::Client::new().post(format!("{}{path}", s.base)).header("content-type", "application/json");
    req = if anthropic {
        req.header("x-api-key", &s.key).header("anthropic-version", "2023-06-01")
    } else {
        req.bearer_auth(&s.key)
    };
    let r = req.body(body.to_string()).send().await.unwrap();
    assert_eq!(r.status(), 200, "{:?}", r.text().await);
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

fn adapter_run(case: &str, rec: &RequestRecord) -> AdapterRun {
    rec.attempts
        .last()
        .and_then(|a| a.adapter.clone())
        .unwrap_or_else(|| panic!("{case}: no adapter run recorded: {rec:?}"))
}

/// SC-009: the run lists exactly these (path, kind, reason) triples.
fn assert_changes(case: &str, run: &AdapterRun, expected: &[(&str, &str, &str)]) {
    assert_eq!(run.outcome, AdapterOutcome::Ran, "{case}: {run:?}");
    let name = |v: Value| v.as_str().unwrap_or_default().to_owned();
    let mut got: Vec<(String, String, String)> = run
        .changes
        .iter()
        .map(|c| {
            (c.path.clone(), name(serde_json::to_value(c.kind).unwrap()), name(serde_json::to_value(c.reason).unwrap()))
        })
        .collect();
    let mut want: Vec<(String, String, String)> =
        expected.iter().map(|(p, k, r)| ((*p).to_owned(), (*k).to_owned(), (*r).to_owned())).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want, "{case}: the record's changes");
}

fn chat_plugin(mock: &nullrouter_engine::testkit::MockUpstream, id: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

fn messages_plugin(mock: &nullrouter_engine::testkit::MockUpstream, id: &str, model: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"anthropic-messages\"\nheaders = {{ \"anthropic-version\" = \"2023-06-01\" }}\nauth = {{ header = \"x-api-key\", scheme = \"raw\" }}\n[[models]]\nid = \"{model}\"\n",
        mock.url(&format!("/{id}/messages"))
    )
}

fn accounts(provider: &str, secret: &str) -> String {
    format!("schema = 1\n[[account]]\nprovider = \"{provider}\"\nname = \"main\"\nsecret = \"{secret}\"\n")
}

// ---------------------------------------------------------------------------------------------
// hermes

#[tokio::test]
async fn hermes_records_hold_no_removed_or_converted_value() {
    let log = logs();
    let case = "hermes";
    let (r1, r2, r3, img, secret) = (
        sentinel(case, 1),
        sentinel(case, 2),
        sentinel(case, 3),
        format!("NRSimg{:x}", std::process::id()),
        sentinel(case, 5),
    );
    let image = png_with(&img);
    let s = server_custom(
        |mock| vec![("groq", chat_plugin(mock, "groq"))],
        &accounts("groq", &secret),
        "[plugin_decisions]\ngroq = \"replace\"\n",
    )
    .await;
    s.mock.respond(reply_by_wire);
    bind(&s, "hermes");

    let calls = json!([{"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}]);
    let body = json!({"model": "groq/m1", "stream": false,
    "tools": [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object", "properties": {}}}}],
    "messages": [
        {"role": "user", "content": "find it"},
        {"role": "assistant", "content": null, "tool_calls": calls,
         "reasoning_content": r1, "reasoning": r2, "reasoning_details": [{"type": "reasoning.text", "text": r3}]},
        {"role": "tool", "tool_call_id": "call_1", "content": "found"},
        {"role": "user", "content": "now this picture", "images": [image.clone()]}
    ]});
    let rec = send(&s, "/v1/chat/completions", false, &body).await;
    assert_changes(
        case,
        &adapter_run(case, &rec),
        &[
            ("messages[1].reasoning_content", "removed", "target_rejects_field"),
            ("messages[1].reasoning", "removed", "target_rejects_field"),
            ("messages[1].reasoning_details", "removed", "target_rejects_field"),
            ("messages[3].content", "converted", "format_conversion"),
            ("messages[3].images", "removed", "format_conversion"),
        ],
    );
    assert_no_sentinel(case, &s, &log, &[r1, r2, r3, img, image, secret]);
}

// ---------------------------------------------------------------------------------------------
// legit_removal

#[tokio::test]
async fn legit_removal_records_hold_no_removed_value() {
    let log = logs();
    let case = "legit_removal";
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nullrouter-adapters/tests/hostile/legit_removal");
    let manifest = std::fs::read_to_string(dir.join("adapter.toml")).unwrap();
    let harness = toml::from_str::<toml::Value>(&manifest).unwrap()["harness"].as_str().unwrap().to_owned();
    let wasm = wat::parse_str(std::fs::read_to_string(dir.join("module.wat")).unwrap()).unwrap();
    let mut body: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("request.json")).unwrap()).unwrap();

    let (max, temp, secret) = (sentinel(case, 1), sentinel(case, 2), sentinel(case, 3));
    body["model"] = json!("mockco/m1");
    body["stream"] = json!(false);
    body["max_tokens"] = json!(max);
    body["temperature"] = json!(temp);

    let s = server_custom(
        |mock| {
            vec![(
                "mockco",
                format!(
                    "schema = 2\nid = \"mockco\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n",
                    mock.url("/v1/chat/completions")
                ),
            )]
        },
        &accounts("mockco", &secret),
        "",
    )
    .await;
    s.mock.respond(reply_by_wire);
    install_fixture(s.home(), &harness, &manifest, &wasm);
    s.engine.open_adapters().unwrap();
    bind(&s, &harness);

    let rec = send(&s, "/v1/chat/completions", false, &body).await;
    assert_changes(
        case,
        &adapter_run(case, &rec),
        &[
            ("max_tokens", "removed", "param_unsupported_by_model"),
            ("temperature", "removed", "param_unsupported_by_model"),
        ],
    );
    assert_no_sentinel(case, &s, &log, &[max, temp, secret]);
}

// ---------------------------------------------------------------------------------------------
// Claude Code

const CC_MODEL: &str = "claude-sonnet-4-5-20250929";

fn claude_fixture() -> Option<Vec<u8>> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nullrouter-adapters/tests/fixtures/claude-code/module.wasm");
    let wasm = std::fs::read(path).ok();
    if wasm.is_none() {
        eprintln!("skipped: build the claude-code fixture (T080)");
    }
    wasm
}

fn oracle(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/9router/adapters/claude-code")
        .join(format!("{name}.in.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut body = v["data"].clone();
    body["model"] = json!(format!("anthropic/{CC_MODEL}"));
    body["stream"] = json!(false);
    body
}

/// Runs the Claude Code adapter over `body`, checks the record's changes, and searches for
/// `sentinels` (plus the account secret).
async fn claude_case(case: &str, body: Value, sentinels: Vec<String>, secret: String, expected: &[(&str, &str, &str)]) {
    let Some(wasm) = claude_fixture() else { return };
    let log = logs();
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../adapters/community/claude-code/adapter.toml");
    let manifest = std::fs::read_to_string(manifest_path).unwrap();
    let s = server_custom(
        |mock| vec![("anthropic", messages_plugin(mock, "anthropic", CC_MODEL))],
        &accounts("anthropic", &secret),
        "[plugin_decisions]\nanthropic = \"replace\"\n",
    )
    .await;
    s.mock.respond(reply_by_wire);
    install_fixture(s.home(), "claude-code", &manifest, &wasm);
    s.engine.open_adapters().unwrap();
    bind(&s, "claude-code");

    let rec = send(&s, "/v1/messages", true, &body).await;
    assert_changes(case, &adapter_run(case, &rec), expected);
    let mut all = sentinels;
    all.push(secret);
    assert_no_sentinel(case, &s, &log, &all);
}

#[tokio::test]
async fn claude_code_foreign_thinking_leaves_no_content() {
    let case = "cc_thinking";
    let (text, sig, secret) = (sentinel(case, 1), sentinel(case, 2), sentinel(case, 3));
    let mut body = oracle("thinking-foreign-signature");
    body["messages"][1]["content"][0]["thinking"] = json!(text);
    body["messages"][1]["content"][0]["signature"] = json!(sig);
    claude_case(case, body, vec![text, sig], secret, &[("messages[1].content[0]", "removed", "foreign_block")]).await;
}

#[tokio::test]
async fn claude_code_foreign_server_tool_blocks_leave_no_content() {
    let case = "cc_server_tool";
    let s: Vec<String> = (1..=5).map(|n| sentinel(case, n)).collect();
    let mut body = oracle("server-tool-use-foreign-id");
    body["messages"][1]["content"][1]["input"]["query"] = json!(s[0]);
    body["messages"][1]["content"][2]["content"][0]["title"] = json!(s[1]);
    body["messages"][3]["content"][0]["input"]["query"] = json!(s[2]);
    body["messages"][4]["content"][0]["content"] = json!(s[3]);
    let secret = s[4].clone();
    claude_case(
        case,
        body,
        s[..4].to_vec(),
        secret,
        &[
            ("messages[1].content[1]", "removed", "foreign_block"),
            ("messages[1].content[2]", "removed", "foreign_block"),
            ("messages[3]", "removed", "empty_after_removal"),
            ("messages[4].content[0]", "removed", "foreign_block"),
        ],
    )
    .await;
}

#[tokio::test]
async fn claude_code_duplicate_tools_leave_no_content() {
    let case = "cc_duplicate_tools";
    let mut body = oracle("duplicate-tools");
    let count = body["tools"].as_array().unwrap().len();
    let sentinels: Vec<String> = (0..count).map(|i| sentinel(case, i)).collect();
    for (i, sn) in sentinels.iter().enumerate() {
        body["tools"][i]["description"] = json!(sn);
    }
    let secret = sentinel(case, 99);
    claude_case(
        case,
        body,
        sentinels,
        secret,
        &[
            ("tools[1]", "removed", "duplicate_tool"),
            ("tools[2]", "removed", "duplicate_tool"),
            ("tools[3]", "removed", "duplicate_tool"),
            ("tools[6]", "removed", "duplicate_tool"),
        ],
    )
    .await;
}
