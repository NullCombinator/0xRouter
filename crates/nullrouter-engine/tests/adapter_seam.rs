//! The adapter seam in the attempt loop (spec 004, T014), with a test adapter that removes every
//! part its selector matches. (e) and (f) drive the client's stream writer the way the server
//! does: the writer's bytes pass through the adapter's tap, one chunk at a time.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use nullrouter_adapter_kit::{Context, Edits, Reason};
use nullrouter_adapters::runner::{AdapterRunner, Fixture, RequestFn};
use nullrouter_adapters::selector::{Selector, extract};
use nullrouter_engine::keys::Keys;
use nullrouter_engine::attempt::{Answer, Media};
use nullrouter_engine::records::{AdapterOutcome, NotRunReason, Outcome, RequestRecord};
use nullrouter_engine::routing::PlacementReason;
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_registry::schema::ModelType;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

type Seen = Arc<Mutex<Vec<Context>>>;

/// A test adapter named `name` that removes every part `selector` matches.
fn remover(name: &str, selector: &str, seen: &Seen) -> AdapterRunner {
    let selectors = vec![Selector::parse(selector).unwrap()];
    let for_run = selectors.clone();
    let seen = seen.clone();
    AdapterRunner::Fixture(Fixture {
        name: name.into(),
        selectors,
        request: Arc::new(move |ctx: &Context, body: &Value| {
            seen.lock().unwrap().push(ctx.clone());
            let mut out = Edits::default();
            for part in extract(body, &for_run) {
                out.remove(&part.path, Reason::TargetRejectsField);
            }
            out
        }),
        ..Default::default()
    })
}

/// Issues a key bound to `harness` (or none) and returns its id. The engine reloads to see it.
fn key(s: &Setup, harness: Option<&str>) -> String {
    let path = s._dir.path().join("keys.toml");
    let mut keys = Keys::load(&path).unwrap();
    let id = keys.issue(&format!("k{}", keys.iter().count()), None).unwrap().1.id.clone();
    if let Some(h) = harness {
        keys.set_harness(&id, Some(nullrouter_engine::keys::HarnessName::new(h).unwrap())).unwrap();
    }
    keys.save().unwrap();
    s.engine.reload_blocking().unwrap();
    id
}

async fn alpha() -> Setup {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    s.mock.push([ok()]);
    s
}

fn body_with_reasoning() -> Value {
    json!({"model": "alpha/m1", "stream": false, "messages": [
        {"role": "user", "content": "hi"},
        {"role": "assistant", "content": "hello", "reasoning_content": "SENTINEL-REASONING"}
    ]})
}

async fn send_body(s: &Setup, client: &str, body: Value, agent: &str) -> RequestRecord {
    let req = request(s, client, "alpha/m1", body, agent, CancellationToken::new());
    let id = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.unwrap();
    settled(s, &id).await
}

#[tokio::test]
async fn a_removed_field_never_reaches_the_upstream_and_the_attempt_lists_it() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, Some("fixture"));
    let rec = send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    let got = s.mock.received();
    assert_eq!(got.len(), 1);
    let sent = String::from_utf8_lossy(&got[0].body).to_string();
    assert!(!sent.contains("SENTINEL-REASONING") && !sent.contains("reasoning_content"), "{sent}");
    assert!(sent.contains("hello"), "the rest of the message goes on: {sent}");

    let run = rec.attempts[0].adapter.as_ref().expect("the attempt records the run");
    assert_eq!(run.outcome, AdapterOutcome::Ran);
    assert_eq!(run.changes.len(), 1);
    assert_eq!(run.changes[0].path, "messages[1].reasoning_content");
    let record = serde_json::to_string(&rec).unwrap();
    assert!(!record.contains("SENTINEL-REASONING"), "a record holds paths, never content");
}

#[tokio::test]
async fn a_key_without_a_harness_runs_no_adapter() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, None);
    let rec = send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    assert!(seen.lock().unwrap().is_empty());
    assert!(rec.attempts[0].adapter.is_none());
    assert!(String::from_utf8_lossy(&s.mock.received()[0].body).contains("SENTINEL-REASONING"));
}

#[tokio::test]
async fn a_harness_with_nothing_installed_runs_as_a_plain_client() {
    let s = alpha().await;
    let id = key(&s, Some("never-installed"));
    let rec = send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    assert!(String::from_utf8_lossy(&s.mock.received()[0].body).contains("SENTINEL-REASONING"));
    let run = rec.attempts[0].adapter.as_ref().unwrap();
    assert!(matches!(run.outcome, AdapterOutcome::NotRun { .. }), "{:?}", run.outcome);
}

#[tokio::test]
async fn the_context_carries_no_header_key_agent_or_session() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, Some("fixture"));
    send_body(&s, "openai-chat", body_with_reasoning(), &id).await;

    let ctxs = seen.lock().unwrap().clone();
    assert_eq!(ctxs.len(), 1);
    let text = serde_json::to_string(&ctxs[0]).unwrap();
    for secret in [id.as_str(), "sess-1", SECRET, "authorization", "Bearer"] {
        assert!(!text.contains(secret), "the context holds {secret:?}: {text}");
    }
    assert_eq!(ctxs[0].provider, "alpha");
    assert_eq!(ctxs[0].target_style, "openai-chat");
    assert!(ctxs[0].same_style);
    assert_eq!(ctxs[0].attempt, 1);
}

#[tokio::test]
async fn a_fallback_runs_the_adapter_again_with_its_own_context_and_changes() {
    let s = setup(
        |m| vec![("alpha", chat_plugin(m, "alpha", "")), ("beta", chat_plugin(m, "beta", ""))],
        &[("alpha", "a1"), ("beta", "b1")],
        &unified(&[("alpha", "m1"), ("beta", "m1")]),
    )
    .await;
    s.mock.on("/alpha", [err(503)]);
    s.mock.on("/beta", [ok()]);
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].reasoning_content", &seen));
    let id = key(&s, Some("fixture"));
    let req = request(&s, "openai-chat", "u", body_with_reasoning(), &id, CancellationToken::new());
    let rid = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.unwrap();
    let rec = settled(&s, &rid).await;

    let ctxs = seen.lock().unwrap().clone();
    let providers: Vec<&str> = ctxs.iter().map(|c| c.provider.as_str()).collect();
    assert!(providers.contains(&"alpha") && providers.contains(&"beta"), "{providers:?}");
    assert!(ctxs.windows(2).all(|w| w[0].attempt < w[1].attempt), "attempt numbers rise: {ctxs:?}");
    for a in rec.attempts.iter().filter(|a| a.adapter.is_some()) {
        assert_eq!(a.adapter.as_ref().unwrap().changes.len(), 1, "each attempt has its own changes");
    }
    assert!(rec.attempts.iter().filter(|a| a.adapter.is_some()).count() >= 2);
}

#[tokio::test]
async fn a_cross_style_attempt_encodes_from_the_edited_request() {
    let s = alpha().await;
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].content[1]", &seen));
    let id = key(&s, Some("fixture"));
    // Messages in, Chat Completions out. The adapter removes the second content block; the
    // upstream body is encoded from the request decoded again from the edited body, so it
    // lacks that block too.
    let body = json!({"model": "alpha/m1", "max_tokens": 16, "messages": [{"role": "user", "content": [
        {"type": "text", "text": "KEEP-ME"},
        {"type": "text", "text": "DROP-ME"}
    ]}]});
    let rec = send_body(&s, "anthropic-messages", body, &id).await;

    let ctxs = seen.lock().unwrap().clone();
    assert_eq!(ctxs.len(), 1);
    assert!(!ctxs[0].same_style);
    assert_eq!(ctxs[0].target_style, "openai-chat");
    let sent = String::from_utf8_lossy(&s.mock.received()[0].body).to_string();
    assert!(sent.contains("KEEP-ME") && !sent.contains("DROP-ME"), "{sent}");
    let run = rec.attempts[0].adapter.as_ref().unwrap();
    assert_eq!(run.outcome, AdapterOutcome::Ran);
    assert_eq!(run.changes[0].path, "messages[0].content[1]");
}

#[tokio::test]
async fn a_token_count_runs_the_request_side() {
    let s = setup(|m| vec![("mockco", messages_plugin(m, "mockco"))], &[("mockco", "main")], "").await;
    // Give the endpoint a count URL on the same host, so the count goes upstream.
    let plugin = format!(
        "{}[endpoints.text.token_count]\nurl = \"{}\"\n",
        messages_plugin(&s.mock, "mockco"),
        s.mock.url("/count")
    );
    std::fs::write(s._dir.path().join("plugins/mockco.toml"), plugin).unwrap();
    s.engine.reload().await.unwrap();
    s.mock.push([Step::json(200, json!({"input_tokens": 5}))]);
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].tag", &seen));
    let id = key(&s, Some("fixture"));

    let body = json!({"model": "mockco/m1", "max_tokens": 8, "messages": [{"role": "user", "content": "hi", "tag": "TAG-SENTINEL"}]});
    let mut req = request(&s, "anthropic-messages", "mockco/m1", body, &id, CancellationToken::new());
    req.count = true;
    let rid = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.unwrap();
    let rec = settled(&s, &rid).await;

    assert_eq!(seen.lock().unwrap().len(), 1, "the request side ran for the count");
    let got = s.mock.received();
    assert_eq!(got[0].path_and_query, "/count");
    assert!(!String::from_utf8_lossy(&got[0].body).contains("TAG-SENTINEL"));
    assert_eq!(rec.attempts[0].adapter.as_ref().unwrap().outcome, AdapterOutcome::Ran);
}

/// A provider with an embeddings endpoint on the openai-chat wire.
fn embeddings_plugin(mock: &MockUpstream) -> String {
    format!(
        "schema = 2\nid = \"alpha\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n\
         [endpoints.embeddings]\nurl = \"{}\"\nwire = \"openai-chat\"\n\
         [[models]]\nid = \"e1\"\nkind = \"embedding\"\n",
        mock.url("/alpha/embeddings")
    )
}

#[tokio::test]
async fn a_media_request_runs_no_adapter_and_records_why() {
    let s = setup(|m| vec![("alpha", embeddings_plugin(m))], &[("alpha", "a1")], "").await;
    s.mock.push([Step::json(
        200,
        json!({"object": "list", "data": [{"object": "embedding", "index": 0, "embedding": [0.5]}], "model": "e1", "usage": {"prompt_tokens": 2, "total_tokens": 2}}),
    )]);
    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "$", &seen));
    let id = key(&s, Some("fixture"));

    let body = json!({"model": "alpha/e1", "input": "hi"});
    let mut req = request(&s, "openai-chat", "alpha/e1", chat_body("alpha/e1", false), &id, CancellationToken::new());
    let codec = req.client.type_codec(ModelType::Embeddings).unwrap().clone();
    let input = codec.decode_request(&body).unwrap();
    req.body = body;
    req.media = Some(Media { ty: ModelType::Embeddings, codec, variant: None, input, voice: None, job: false });
    let rid = req.id.clone();
    let Ok(Answer::Media(_)) = s.engine.text(s.engine.snapshot(), req).await else { panic!("expected a media answer") };
    let rec = settled(&s, &rid).await;

    assert!(seen.lock().unwrap().is_empty(), "no adapter call for a media request");
    assert_eq!(
        rec.attempts[0].adapter.as_ref().unwrap().outcome,
        AdapterOutcome::NotRun { reason: NotRunReason::MediaRequest }
    );
    assert_eq!(s.mock.received()[0].json()["input"], "hi", "the request goes on unedited");
}

const SUB: &str = "[routing.cache]\nmode = \"automatic\"\nlifetime = \"5m\"\nmin_tokens = 0\n\n[[routing.window]]\nname = \"5h\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 1000000\n";

fn turns(turns: &[&str]) -> Value {
    let mut messages = vec![json!({"role": "system", "content": "You are a careful assistant. Answer in one short sentence and never guess."})];
    for (i, t) in turns.iter().enumerate() {
        messages.push(json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": t}));
    }
    json!({"model": "u", "stream": false, "messages": messages})
}

#[tokio::test]
async fn routing_fingerprints_the_request_as_received_so_a_changed_adapter_still_finds_the_warm_account() {
    let f = Fleet::new()
        .provider("alpha", SUB, &[("5h", "tokens")])
        .account("alpha", "one", 1.0)
        .account("alpha", "two", 1.0)
        .unified(&[("alpha", "m1")])
        .build()
        .await;
    let s = &f.setup;
    // Turn one goes through an adapter that rewrites the first user turn; turn two through one
    // that does nothing. If routing fingerprinted the edited body, the two chains would differ
    // and the second request would miss the account that holds the cache.
    let rewrite = AdapterRunner::Fixture(Fixture {
        name: "fixture".into(),
        selectors: vec![Selector::parse("messages[1].content").unwrap()],
        request: Arc::new(|_: &Context, _: &Value| {
            let mut e = Edits::default();
            e.convert(&nullrouter_adapter_kit::Path::parse("messages[1].content").unwrap(), json!("REWRITTEN"), Reason::FormatConversion);
            e
        }),
        ..Default::default()
    });
    s.engine.install_runner("fixture", rewrite);
    let id = key(s, Some("fixture"));

    let ask = |t: Vec<&'static str>| {
        let req = request(s, "openai-chat", "u", turns(&t), &id, CancellationToken::new());
        let rid = req.id.clone();
        async move {
            s.engine.text(s.engine.snapshot(), req).await.unwrap();
            settled(s, &rid).await
        }
    };
    let first = ask(vec!["one"]).await;
    let sent = String::from_utf8_lossy(&s.mock.received()[0].body).to_string();
    assert!(sent.contains("REWRITTEN") && !sent.contains("\"one\""), "the adapter edited what went upstream: {sent}");

    let seen = Seen::default();
    s.engine.install_runner("fixture", remover("fixture", "messages[*].never", &seen));
    let second = ask(vec!["one", "two", "three"]).await;

    let account = |r: &RequestRecord| r.served_by.as_ref().and_then(|x| x.account.clone());
    assert_eq!(account(&first), account(&second), "the follow-up stays on the warm account");
    assert_eq!(second.attempts.last().and_then(|a| a.placement).unwrap().reason, PlacementReason::Warm);
}

/// A response-side test adapter: upper-cases the text of every chat delta it sees.
fn shouter(name: &str) -> AdapterRunner {
    let response_selectors = vec![Selector::parse("choices[*].delta.content").unwrap()];
    let for_run = response_selectors.clone();
    let event: RequestFn = Arc::new(move |_: &Context, ev: &Value| {
        let mut out = Edits::default();
        for part in extract(ev, &for_run) {
            if let Some(t) = part.value.as_str() {
                out.convert(&part.path, json!(t.to_uppercase()), Reason::FormatConversion);
            }
        }
        out
    });
    AdapterRunner::Fixture(Fixture { name: name.into(), response_selectors, event: Some(event), ..Default::default() })
}

/// One content frame, then the upstream goes quiet for 30 s: nothing else is sent unless the
/// client's chunk got out before it.
fn one_frame_then_silence() -> Step {
    let frame = format!("data: {}\n\n", json!({"id": "c1", "choices": [{"index": 0, "delta": {"content": "hel"}}]}));
    Step::StallAfter { frames: vec![frame.into()], hold: Duration::from_secs(30) }
}

/// Starts a streamed request from a key bound to the shouter, and writes the first piece the
/// way the server does. Returns the open receiver, the client bytes of that piece, the
/// record id and the cancel token.
async fn first_chunk(
    s: &Setup,
) -> (tokio::sync::mpsc::Receiver<nullrouter_engine::attempt::Piece>, nullrouter_engine::response_side::Tap, String, String, CancellationToken)
{
    use nullrouter_engine::attempt::Piece;
    use nullrouter_wire::stream::StreamWriter;

    s.mock.push([one_frame_then_silence()]);
    s.engine.install_runner("shouter", shouter("shouter"));
    let id = key(s, Some("shouter"));
    let body = json!({"model": "alpha/m1", "stream": true, "messages": [{"role": "user", "content": "hi"}]});
    let cancel = CancellationToken::new();
    let req = request(s, "openai-chat", "alpha/m1", body.clone(), &id, cancel.clone());
    let (rid, client) = (req.id.clone(), req.client.clone());
    let reply = s.engine.reply(s.engine.snapshot(), req).await.unwrap();
    let side = reply.response.expect("an adapter that reads responses gives the writer a side");
    let Answer::Events { mut rx, .. } = reply.answer else { panic!("events") };
    let mut tap = side.tap(nullrouter_registry::schema::Framing::SseData).expect("a tap for SSE");
    let mut w = StreamWriter::new(&client, &body, &rid, "", 0).unwrap();
    // Pieces up to the first content: the writer's bytes, each through the tap.
    let mut text = String::new();
    while !text.contains("data:") || !text.to_lowercase().contains("hel") {
        let out = match rx.recv().await.expect("the stream is open") {
            Piece::Event(ev) => w.write(&ev),
            Piece::Frame(f, events) => {
                w.observe(&events);
                f.to_bytes(nullrouter_registry::schema::Framing::SseData).unwrap_or_default()
            }
            Piece::Restart => w.restart(),
        };
        if !out.is_empty() {
            text += &tap.push(out);
        }
    }
    (rx, tap, text, rid, cancel)
}

#[tokio::test]
async fn a_response_adapter_edits_each_event_before_the_next_one_is_read() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    let started = std::time::Instant::now();
    let (_rx, tap, text, _rid, cancel) = first_chunk(&s).await;
    // The upstream is silent for 30 s, so this chunk got to the client without waiting for it.
    assert!(started.elapsed() < Duration::from_secs(10), "the first event waited for the next: {:?}", started.elapsed());
    assert!(text.contains("HEL") && !text.contains("hel"), "the client has the adapter's edit: {text}");

    let run = tap.finish().expect("the adapter ran on the events");
    assert_eq!(run.outcome, AdapterOutcome::Ran);
    assert!(run.changes.iter().all(|c| c.path.starts_with("event[") && c.path.ends_with("choices[0].delta.content")), "{run:?}");
    assert!(!format!("{run:?}").contains("HEL"), "a record holds paths, never content");
    cancel.cancel();
}

#[tokio::test]
async fn a_client_that_goes_while_the_adapter_is_working_cancels_the_upstream() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    let (rx, _tap, text, rid, cancel) = first_chunk(&s).await;
    assert!(text.contains("HEL"));
    // The server's body guard cancels the request when the client goes; the receiver goes too.
    drop(rx);
    cancel.cancel();
    let r = settled(&s, &rid).await;
    assert_eq!(r.outcome, Outcome::Cancelled);
    for _ in 0..200 {
        if !s.mock.disconnects().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the upstream never saw the connection close");
}

#[tokio::test]
async fn a_whole_answer_goes_through_the_response_side_once() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "a1")], "").await;
    s.mock.push([Step::json(
        200,
        json!({"id": "c1", "object": "chat.completion", "model": "m1",
               "choices": [{"index": 0, "message": {"role": "assistant", "content": "hello"}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
    )]);
    let whole: RequestFn = Arc::new(|_: &Context, body: &Value| {
        let mut out = Edits::default();
        out.convert(&nullrouter_adapter_kit::Path::parse("choices[0].message.content").unwrap(), body["choices"][0]["message"]["content"].as_str().map(|t| json!(t.to_uppercase())).unwrap(), Reason::FormatConversion);
        out
    });
    s.engine.install_runner(
        "shouter",
        AdapterRunner::Fixture(Fixture {
            name: "shouter".into(),
            response_selectors: vec![Selector::parse("choices[*].message.content").unwrap()],
            response: Some(whole),
            ..Default::default()
        }),
    );
    let id = key(&s, Some("shouter"));
    let req = request(&s, "openai-chat", "alpha/m1", chat_body("alpha/m1", false), &id, CancellationToken::new());
    let rid = req.id.clone();
    let Answer::Whole { raw, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else { panic!("whole") };
    let sent: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(sent["choices"][0]["message"]["content"], "HELLO");
    let rec = settled(&s, &rid).await;
    let run = rec.response_adapter.expect("the response side is recorded");
    assert_eq!(run.outcome, AdapterOutcome::Ran);
    assert_eq!(run.changes.len(), 1);
    assert_eq!(run.changes[0].path, "choices[0].message.content");
}
