//! The hostile adapter corpus through the server (spec 004, T066; SC-002, SC-003, US3-1 to
//! US3-8). Each case under `nullrouter-adapters/tests/hostile/` is installed as an approved
//! version, bound to a key, and run on whole and streamed requests against a mock upstream.
//!
//! "Unmodified" is judged against a control: the same request sent in the same server with a
//! second key that has no adapter. Request side compares the body the mock received; response
//! and event sides compare what the client got (the choices, or the streamed text).

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{Server, reply_by_wire, server_custom};
use nullrouter_adapters::alerts::AlertLog;
use nullrouter_adapters::record::{AdapterOutcome, AdapterRun};
use nullrouter_adapters::store::{Store, VersionState};
use nullrouter_adapters::testkit::install_fixture;
use nullrouter_engine::keys::{HarnessName, Keys};
use nullrouter_engine::records::RequestRecord;
use nullrouter_engine::testkit::Step;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const SENTINEL: &str = "NR-SENTINEL-";

struct Expect {
    outcome: String,
    detail: Option<String>,
    side: String,
    alert: Option<String>,
    suspect: bool,
    input: String,
    changes: Option<usize>,
}

fn expect_of(dir: &std::path::Path) -> Expect {
    let text = std::fs::read_to_string(dir.join("expect.toml")).unwrap();
    let t: toml::Value = toml::from_str(&text).unwrap();
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    Expect {
        outcome: s("outcome").expect("outcome"),
        detail: s("detail"),
        side: s("side").expect("side"),
        alert: s("alert"),
        suspect: t.get("suspect").and_then(toml::Value::as_bool).unwrap_or(false),
        input: s("input").expect("input"),
        changes: t.get("changes").and_then(toml::Value::as_integer).map(|n| usize::try_from(n).unwrap()),
    }
}

fn content(text: &str) -> String {
    text.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .filter_map(|v| v["choices"][0]["delta"]["content"].as_str().map(str::to_owned))
        .collect()
}

/// What the client sees of an answer, reduced to what an adapter could have changed.
fn visible(text: &str, stream: bool) -> Value {
    if stream {
        json!(content(text))
    } else {
        serde_json::from_str::<Value>(text).map_or(Value::Null, |v| v["choices"].clone())
    }
}

async fn send(s: &Server, key: &str, stream: bool, body: &Value) -> (String, RequestRecord) {
    let mut body = body.clone();
    body["stream"] = json!(stream);
    let r = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(key)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let text = r.text().await.unwrap();
    for _ in 0..200 {
        if let Some(rec) = s.engine.records.get(&id).filter(|r| r.total_ms.is_some()) {
            return (text, rec);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the record never settled: {:?}", s.engine.records.get(&id));
}

fn check_run(case: &str, run: &AdapterRun, e: &Expect, suspect_now: bool) {
    if suspect_now {
        assert!(
            matches!(
                run.outcome,
                AdapterOutcome::NotRun { reason: nullrouter_adapters::record::NotRunReason::Suspect }
            ),
            "{case}: a suspect version must not run: {run:?}"
        );
        return;
    }
    let detail = e.detail.as_deref();
    match e.outcome.as_str() {
        "ran" => {
            assert_eq!(run.outcome, AdapterOutcome::Ran, "{case}: {run:?}");
            if let Some(n) = e.changes {
                assert_eq!(run.changes.len(), n, "{case}: {run:?}");
            }
        }
        "not_run" => {
            let AdapterOutcome::NotRun { reason } = &run.outcome else { panic!("{case}: expected not_run: {run:?}") };
            assert_eq!(serde_json::to_value(reason).unwrap().as_str(), detail, "{case}: {run:?}");
        }
        "failed" => {
            let AdapterOutcome::Failed { reason } = &run.outcome else { panic!("{case}: expected failed: {run:?}") };
            assert_eq!(reason.codes().last().copied(), detail, "{case}: {run:?}");
        }
        "blocked" => {
            assert_eq!(run.outcome, AdapterOutcome::Blocked, "{case}: {run:?}");
            let g = run.guardrail.as_ref().unwrap_or_else(|| panic!("{case}: no guardrail event: {run:?}"));
            assert_eq!(g.rule.codes().first().copied(), detail, "{case}: {run:?}");
        }
        other => panic!("{case}: unknown outcome {other}"),
    }
}

async fn run_case(case: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nullrouter-adapters/tests/hostile").join(case);
    let e = expect_of(&dir);
    let manifest = std::fs::read_to_string(dir.join("adapter.toml")).unwrap();
    let harness = toml::from_str::<toml::Value>(&manifest).unwrap()["harness"].as_str().unwrap().to_owned();
    let wasm = wat::parse_str(std::fs::read_to_string(dir.join("module.wat")).unwrap()).unwrap();
    let input: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(&e.input)).unwrap()).unwrap();

    let secret = format!(
        "{SENTINEL}{:x}{:x}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        std::process::id()
    );
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
        &format!("schema = 1\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"{secret}\"\n"),
        "",
    )
    .await;

    // A decoy listener: nothing is pointed at it, so any accept is a connection the process
    // opened on its own.
    let decoy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    tokio::spawn(async move {
        while decoy.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let id = install_fixture(s.home(), &harness, &manifest, &wasm);
    s.engine.open_adapters().unwrap();
    let path = s.home().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    let name = HarnessName::new(&harness).unwrap();
    keys.set_adapter("laptop", Some(name.clone())).unwrap();
    let (plain, _) = keys.issue("plain", None).unwrap();
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();

    // What the upstream answers, and the modes to run.
    let chat_body = |msgs: Value| json!({"model": "mockco/m1", "messages": msgs});
    let (client_body, modes): (Value, &[bool]) = match e.side.as_str() {
        "request" => {
            let mut b = input.clone();
            b["model"] = json!("mockco/m1");
            s.mock.respond(reply_by_wire);
            (b, &[false, true][..])
        }
        "response" => {
            let answer = input.clone();
            s.mock.respond(move |_| Step::json(200, answer.clone()));
            (chat_body(json!([{"role": "user", "content": "hi"}])), &[false][..])
        }
        "event" => {
            let finish = json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1, "model": "m1",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
            let first = input.clone();
            s.mock.respond(move |_| Step::sse(&[(None, first.clone()), (None, finish.clone())], true));
            (chat_body(json!([{"role": "user", "content": "hi"}])), &[true][..])
        }
        other => panic!("{case}: unknown side {other}"),
    };

    let mut blocked_before = false;
    let mut sent = 0;
    // The last mode runs once more for a guardrail case, to see the version stopped serving.
    let mut rounds: Vec<bool> = modes.to_vec();
    if e.suspect {
        rounds.push(*modes.last().unwrap());
    }
    for stream in rounds {
        let (base_text, _) = send(&s, &plain, stream, &client_body).await;
        let (text, rec) = send(&s, &s.key, stream, &client_body).await;
        sent += 2;
        let got = s.mock.received();
        assert_eq!(got.len(), sent, "{case}: the mock saw an unexpected number of requests");
        let suspect_now = blocked_before;

        let run = match e.side.as_str() {
            "request" => rec.attempts.last().and_then(|a| a.adapter.clone()),
            _ if suspect_now => rec.attempts.first().and_then(|a| a.adapter.clone()),
            _ => rec.response_adapter.clone(),
        }
        .unwrap_or_else(|| panic!("{case}: no adapter run recorded: {rec:?}"));
        check_run(case, &run, &e, suspect_now);

        // The request or response went through as it would have with no adapter.
        let changed = e.changes.unwrap_or(0) > 0 && !suspect_now;
        if e.side == "request" {
            let (base, with) = (got[sent - 2].json(), got[sent - 1].json());
            if changed {
                assert_ne!(base, with, "{case}: the accepted edits did not reach the upstream");
                assert!(with.get("max_tokens").is_none() && with.get("temperature").is_none(), "{case}: {with}");
            } else {
                assert_eq!(base, with, "{case}: the upstream body differs from the control (stream={stream})");
            }
        } else {
            assert_eq!(visible(&base_text, stream), visible(&text, stream), "{case}: the client answer differs");
            assert!(!text.contains("run_shell"), "{case}: the added tool call reached the client: {text}");
        }
        if e.outcome == "blocked" && !suspect_now {
            blocked_before = true;
        }
    }

    // No sentinel in any body the upstream saw; none in the records or alerts either.
    for r in s.mock.received() {
        assert!(!String::from_utf8_lossy(&r.body).contains(SENTINEL), "{case}: the upstream body carries the secret");
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "{case}: the decoy listener was reached");

    let alerts = AlertLog::open(&Store::open(s.home()).unwrap()).list().unwrap();
    assert!(!format!("{alerts:?}").contains(SENTINEL), "{case}: an alert carries the secret");
    match &e.alert {
        Some(kind) => {
            let hit = alerts.iter().filter(|a| a.harness == name && a.kind.to_string() == *kind).collect::<Vec<_>>();
            assert!(!hit.is_empty(), "{case}: no {kind} alert raised: {alerts:?}");
            // A refused module's detail is a load code, not the run's not-run reason.
            if kind != "module_refused" {
                let d = e.detail.as_deref().unwrap();
                assert!(hit.iter().any(|a| a.detail.contains(d)), "{case}: no alert names {d}: {hit:?}");
            }
        }
        None => assert!(alerts.is_empty(), "{case}: unexpected alerts {alerts:?}"),
    }

    // Only a guardrail block leaves the version suspect.
    let index = Store::open(s.home()).unwrap().load_index().unwrap();
    let state = index.version(&name, &id).expect("the version stays in the index").state;
    let want = if e.suspect { VersionState::Suspect } else { VersionState::Approved };
    assert_eq!(state, want, "{case}: version state");
}

macro_rules! cases {
    ($($f:ident => $dir:literal),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $f() {
                run_case($dir).await;
            }
        )*
    };
}

cases! {
    add_tool_call_event => "add_tool_call_event",
    add_tool_call_request => "add_tool_call_request",
    add_tool_call_response => "add_tool_call_response",
    add_tool_def => "add_tool_def",
    add_unplaced_field => "add_unplaced_field",
    bad_output => "bad_output",
    change_tool_args => "change_tool_args",
    env_import => "env",
    file_import => "file",
    legit_removal => "legit_removal",
    spin_forever => "loop",
    memory_bomb => "memory_bomb",
    net_import => "net",
    out_of_selector => "out_of_selector",
    rewrite_tool_result => "rewrite_tool_result",
    secret_probe => "secret_probe",
    trap => "trap",
}
