//! The happy-path attempt (T055) against a scripted upstream: same-style and cross-style
//! bodies, streamed and whole answers, `force_stream`, cancellation and the record.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nullrouter_engine::attempt::{self, Answer, Piece, TextRequest};
use nullrouter_engine::keys::AgentId;
use nullrouter_engine::records::{AttemptOutcome, ErrorClass, Outcome, RequestRecord};
use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::OperatorHome;
use nullrouter_wire::codec::request;
use nullrouter_wire::codec::response::ForClient;
use nullrouter_wire::ir::Event;
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const SECRET: &str = "sk-mock-SENTINEL-0001";

struct Setup {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    mock: MockUpstream,
}

async fn setup() -> Setup {
    let mock = MockUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    let plugin = format!(
        r#"schema = 2
id = "mockco"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.text]
url = "{chat}"
wire = "openai-chat"
[session]
header = "x-mock-session"
derive = "ses_sha256_hex32"
[[models]]
id = "m1"
"#,
        chat = mock.url("/v1/chat/completions")
    );
    std::fs::write(dir.path().join("plugins/mockco.toml"), plugin).unwrap();
    let forced = format!(
        "schema = 2\nid = \"forced\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\nforce_stream = true\n",
        mock.url("/forced/chat/completions")
    );
    std::fs::write(dir.path().join("plugins/forced.toml"), forced).unwrap();
    let accounts = format!(
        "schema = 1\n[[account]]\nprovider = \"mockco\"\nname = \"main\"\nsecret = \"{SECRET}\"\n[[account]]\nprovider = \"forced\"\nname = \"main\"\nsecret = \"{SECRET}\"\n"
    );
    nullrouter_engine::files::write_private(&dir.path().join(nullrouter_engine::accounts::FILE), &accounts).unwrap();
    let (engine, report) = Engine::open(OperatorHome::new(dir.path())).unwrap();
    assert!(report.registry.diagnostics.is_empty(), "{:#?}", report.registry.diagnostics);
    Setup { _dir: dir, engine: Arc::new(engine), mock }
}

fn request(s: &Setup, client: &str, target: &str, body: Value, cancel: CancellationToken) -> TextRequest {
    let st = s.engine.snapshot();
    let client = st.style(client).unwrap().clone();
    let ir = request::decode(&client, &body).unwrap();
    let id = nullrouter_engine::records::new_id();
    s.engine.records.insert(RequestRecord::new(id.clone(), "2026-09-28T00:00:00Z".into(), client.id.clone()));
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    TextRequest {
        id,
        arrived: Instant::now(),
        client,
        body,
        ir,
        headers: HeaderMap::new(),
        agent: AgentId::new("ak_test", Some("sess-1")),
        target: target.into(),
        stream,
        cancel,
        media: None,
        count: false,
    }
}

fn chat_chunks() -> Step {
    let chunk = |delta: Value, finish: Value| {
        (
            None,
            json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
        )
    };
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": "Hel"}), Value::Null),
            chunk(json!({"content": "lo"}), Value::Null),
            chunk(json!({}), json!("stop")),
            (
                None,
                json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}}),
            ),
        ],
        true,
    )
}

async fn drain(rx: &mut tokio::sync::mpsc::Receiver<Piece>) -> Vec<Piece> {
    let mut out = Vec::new();
    while let Some(p) = rx.recv().await {
        out.push(p);
    }
    out
}

async fn settled(s: &Setup, id: &str) -> RequestRecord {
    for _ in 0..200 {
        let r = s.engine.records.get(id).unwrap();
        if r.outcome != Outcome::InProgress {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("record {id} never finished");
}

#[tokio::test]
async fn same_style_whole_answer_is_the_clients_body_edited() {
    let s = setup().await;
    s.mock.push([Step::json(
        200,
        json!({"id": "x", "object": "chat.completion", "model": "m1", "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    )]);
    let body = json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}], "seed": 7, "x_unknown": {"kept": true}});
    let req = request(&s, "openai-chat", "mockco/m1", body, CancellationToken::new());
    let id = req.id.clone();
    let Answer::Whole { status, answer, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("whole")
    };
    assert_eq!(status, 200);
    assert!(matches!(*answer, ForClient::AsReceived { read: Some(_) }), "{answer:?}");

    let sent = &s.mock.received()[0];
    let b = sent.json();
    assert_eq!(b["model"], "m1", "the upstream id replaces the target");
    assert_eq!(b["x_unknown"], json!({"kept": true}), "same style passes unknown keys through");
    assert!(b.get("stream_options").is_none(), "no usage switch on a whole answer");
    assert_eq!(sent.headers["authorization"], format!("Bearer {SECRET}"));
    let session = sent.headers["x-mock-session"].to_str().unwrap();
    assert!(session.starts_with("ses_") && session.len() == 36, "{session}");

    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert_eq!(r.served_by.as_ref().unwrap().provider, "mockco");
    assert_eq!(r.attempts.len(), 1);
    assert_eq!(r.attempts[0].outcome, Some(AttemptOutcome::Ok));
    assert_eq!(r.usage.unwrap().output, Some(1));
}

#[tokio::test]
async fn cross_style_stream_turns_into_events_with_usage() {
    let s = setup().await;
    s.mock.push([chat_chunks()]);
    let body = json!({"model": "mockco/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}], "metadata": {"user_id": "u"}});
    let req = request(&s, "anthropic-messages", "mockco/m1", body, CancellationToken::new());
    let id = req.id.clone();
    let Answer::Events { mut rx, forced } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    assert!(!forced);
    let events = drain(&mut rx).await;
    let text: String = events
        .iter()
        .filter_map(|e| if let Piece::Event(Event::TextDelta(t)) = e { Some(t.as_str()) } else { None })
        .collect();
    assert_eq!(text, "Hello");

    let b = s.mock.received()[0].json();
    assert_eq!(b["model"], "m1");
    assert_eq!(b["stream"], true);
    assert_eq!(b["stream_options"]["include_usage"], true, "usage asked for on a streamed chat wire");
    assert_eq!(b["messages"][0]["content"], "hi");

    let r = settled(&s, &id).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert!(r.ttft_ms.is_some());
    let u = r.usage.unwrap();
    assert_eq!((u.input, u.output), (Some(5), Some(2)));
}

#[tokio::test]
async fn a_forced_stream_is_collected_for_a_whole_client() {
    let s = setup().await;
    s.mock.push([chat_chunks()]);
    let body = json!({"model": "forced/m9", "messages": [{"role": "user", "content": "hi"}]});
    let req = request(&s, "openai-chat", "forced/m9", body.clone(), CancellationToken::new());
    let client = req.client.clone();
    let Answer::Events { rx, forced } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    assert!(forced);
    let b = s.mock.received()[0].json();
    assert_eq!(b["stream"], true, "force_stream makes the upstream request a stream");
    let resp = attempt::collect(&client, &body, rx).await.unwrap();
    assert_eq!(resp.text(), "Hello");
}

#[tokio::test]
async fn a_provider_error_fails_the_record_with_its_class() {
    let s = setup().await;
    s.mock.push([Step::json(400, json!({"error": {"message": "bad field", "type": "invalid_request_error"}}))]);
    let body = json!({"model": "mockco/m1", "messages": [{"role": "user", "content": "hi"}]});
    let req = request(&s, "openai-chat", "mockco/m1", body, CancellationToken::new());
    let id = req.id.clone();
    let Err(f) = s.engine.text(s.engine.snapshot(), req).await else { panic!("failure") };
    assert_eq!(f.status, 400);
    assert!(f.message.starts_with(&format!("bad field (record {id})")), "{}", f.message);
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(r.outcome, Outcome::Failed);
    assert!(matches!(r.attempts[0].outcome, Some(AttemptOutcome::Failed { class: ErrorClass::RequestError, .. })));
}

#[tokio::test]
async fn an_unknown_target_fails_before_any_attempt() {
    let s = setup().await;
    let body = json!({"model": "nope/x", "messages": [{"role": "user", "content": "hi"}]});
    let req = request(&s, "openai-chat", "nope/x", body, CancellationToken::new());
    let id = req.id.clone();
    let Err(f) = s.engine.text(s.engine.snapshot(), req).await else { panic!("failure") };
    assert_eq!(f.status, 404);
    assert!(s.mock.received().is_empty());
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!((r.outcome, r.attempts.len()), (Outcome::Failed, 0));
}

#[tokio::test]
async fn cancelling_stops_the_upstream_stream_and_the_record_says_so() {
    let s = setup().await;
    let first = format!("data: {}\n\n", json!({"choices": [{"index": 0, "delta": {"content": "Hel"}}]}));
    s.mock.push([Step::StallAfter { frames: vec![first.into()], hold: Duration::from_secs(30) }]);
    let cancel = CancellationToken::new();
    let body = json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]});
    let req = request(&s, "openai-chat", "mockco/m1", body, cancel.clone());
    let id = req.id.clone();
    let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    let first = rx.recv().await.unwrap();
    assert!(matches!(first, Piece::Frame(..)), "a native stream relays frames: {first:?}");
    cancel.cancel();
    let r = settled(&s, &id).await;
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert_eq!(r.attempts[0].outcome, Some(AttemptOutcome::Cancelled));
    for _ in 0..200 {
        if !s.mock.disconnects().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the upstream never saw the connection close");
}

#[tokio::test]
async fn a_native_stream_relays_the_providers_frames_unchanged() {
    let s = setup().await;
    s.mock.push([chat_chunks(), chat_chunks()]);
    let frames = |pieces: Vec<Piece>| -> Vec<Value> {
        pieces
            .into_iter()
            .map(|p| match p {
                Piece::Frame(f, _) if f.is_done() => json!("[DONE]"),
                Piece::Frame(f, _) => serde_json::from_str(&f.data).unwrap(),
                Piece::Event(e) => panic!("a native stream sent an IR event: {e:?}"),
                Piece::Restart => panic!("a restart without a break"),
            })
            .collect()
    };

    // The client didn't ask for usage: the chunk 0router's switch brought in stays behind.
    let body = json!({"model": "mockco/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]});
    let req = request(&s, "openai-chat", "mockco/m1", body, CancellationToken::new());
    let id = req.id.clone();
    let Answer::Events { mut rx, forced: false } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    let got = frames(drain(&mut rx).await);
    assert_eq!(got.len(), 4, "{got:#?}");
    assert_eq!(
        got[0]["choices"][0]["delta"],
        json!({"role": "assistant", "content": "Hel"}),
        "the provider's chunk as sent"
    );
    assert_eq!(got[0]["object"], "chat.completion.chunk");
    assert_eq!(got[3], "[DONE]");
    assert!(got.iter().all(|v| v.get("usage").is_none()));
    let u = settled(&s, &id).await.usage.unwrap();
    assert_eq!((u.input, u.output), (Some(5), Some(2)), "the record still reads the usage");

    // The client asked: the usage chunk goes through too.
    let body = json!({"model": "mockco/m1", "stream": true, "stream_options": {"include_usage": true}, "messages": [{"role": "user", "content": "hi"}]});
    let req = request(&s, "openai-chat", "mockco/m1", body, CancellationToken::new());
    let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    let got = frames(drain(&mut rx).await);
    assert_eq!(got.len(), 5);
    assert_eq!(got[3]["usage"]["prompt_tokens"], 5);
}
