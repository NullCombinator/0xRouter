//! Mid-stream breaks (T089, research R9): a stream cut after content is continued where
//! the target can take a prefill, else restarted after a note or ended with the error
//! event, as the key (else the operator) chose. Each case runs in every client style.

mod common;

use std::time::{Duration, Instant};

use common::*;
use nullrouter_engine::attempt::{Answer, Piece, TextRequest};
use nullrouter_engine::keys::{AgentId, BreakBehaviour, Keys};
use nullrouter_engine::records::{AttemptKind, BreakHandling, Outcome, RequestRecord};
use nullrouter_engine::testkit::Step;
use nullrouter_wire::codec::{Style, request};
use nullrouter_wire::ir::Event;
use nullrouter_wire::stream::{RESTART_NOTE, StreamWriter};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const STYLES: [&str; 4] = ["openai-chat", "anthropic-messages", "openai-responses", "gemini"];

const PREFILL: &str =
    "[endpoints.text.continuation]\nmethod = \"assistant_prefill\"\ntrim_trailing_whitespace = true\n";

fn chunk(delta: Value, finish: Value) -> (Option<&'static str>, Value) {
    (
        None,
        json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}),
    )
}

fn usage(input: u64, output: u64) -> (Option<&'static str>, Value) {
    (
        None,
        json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [], "usage": {"prompt_tokens": input, "completion_tokens": output}}),
    )
}

/// `n` text deltas `w0 `, `w1 `, …, then the usage so far, cut before the stream ends.
fn cut_after_deltas(n: usize) -> Step {
    let mut events: Vec<_> = (0..n).map(|i| chunk(json!({"content": format!("w{i} ")}), Value::Null)).collect();
    events.push(usage(5, n as u64));
    let len = events.len();
    spaced(Step::sse(&events, true).cut_after(len))
}

/// Frames sent 5 ms apart, so a scripted cut can't overtake the frames before it.
fn spaced(step: Step) -> Step {
    match step {
        Step::Stream { status, headers, frames, cut, .. } => {
            Step::Stream { status, headers, frames, every: Duration::from_millis(5), cut }
        }
        other => other,
    }
}

/// A whole answer `text`.
fn answer(text: &str) -> Step {
    Step::sse(
        &[
            chunk(json!({"role": "assistant", "content": text}), Value::Null),
            chunk(json!({}), json!("stop")),
            usage(30, 3),
        ],
        true,
    )
}

/// A tool call whose arguments are cut half-way.
fn cut_in_tool_call() -> Step {
    let call = |args: &str, head: bool| {
        let mut c = json!({"index": 0, "function": {"arguments": args}});
        if head {
            c["id"] = json!("call_1");
            c["type"] = json!("function");
            c["function"]["name"] = json!("weather");
        }
        chunk(json!({"tool_calls": [c]}), Value::Null)
    };
    spaced(
        Step::sse(&[chunk(json!({"content": "Checking. "}), Value::Null), call("", true), call("{\"ci", false)], true)
            .cut_after(3),
    )
}

fn body(style: &str, target: &str, thinking: bool) -> Value {
    let mut b = match style {
        "openai-chat" => json!({"model": target, "stream": true, "messages": [{"role": "user", "content": "hi"}]}),
        "anthropic-messages" => {
            json!({"model": target, "stream": true, "max_tokens": 100, "messages": [{"role": "user", "content": "hi"}]})
        }
        "openai-responses" => json!({"model": target, "stream": true, "input": "hi"}),
        _ => json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}),
    };
    if thinking {
        b["thinking"] = json!({"type": "enabled", "budget_tokens": 1024});
    }
    b
}

fn streaming(s: &Setup, style: &str, target: &str, agent: &str, thinking: bool) -> TextRequest {
    let st = s.engine.snapshot();
    let client = st.style(style).unwrap().clone();
    let body = body(style, target, thinking);
    let ir = request::decode(&client, &body).unwrap();
    let id = nullrouter_engine::records::new_id();
    s.engine.records.insert(RequestRecord::new(id.clone(), "2026-09-29T00:00:00Z".into(), client.id.clone()));
    TextRequest {
        id,
        arrived: Instant::now(),
        client,
        body,
        ir,
        headers: Default::default(),
        agent: AgentId::new(agent, Some("sess-1")),
        target: target.into(),
        stream: true,
        cancel: CancellationToken::new(),
        media: None,
        count: false,
        pin: None,
        test: None,
    }
}

/// Runs one streaming request; the pieces, the client's bytes and the settled record.
async fn run(s: &Setup, style: &str, target: &str, agent: &str, thinking: bool) -> (Vec<Piece>, String, RequestRecord) {
    let req = streaming(s, style, target, agent, thinking);
    let (id, client, body) = (req.id.clone(), req.client.clone(), req.body.clone());
    let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    let pieces = drain(&mut rx).await;
    let bytes = render(&client, &body, &pieces);
    (pieces, bytes, settled(s, &id).await)
}

/// The client's bytes, as the server writes them.
fn render(client: &Style, body: &Value, pieces: &[Piece]) -> String {
    let framing = client.text().unwrap().framing;
    let mut w = StreamWriter::new(client, body, "rq_test", "", 0).unwrap();
    let mut out = String::new();
    let mut relayed = false;
    for p in pieces {
        match p {
            Piece::Event(ev) => {
                relayed &= !ev.is_output();
                out += &w.write(ev);
            }
            Piece::Frame(f, events) => {
                w.observe(events);
                relayed = true;
                out += &f.to_bytes(framing).unwrap_or_default();
            }
            Piece::Restart => {
                relayed = false;
                out += &w.restart();
            }
        }
    }
    if !relayed {
        out += &w.end();
    }
    out
}

/// The answer text the client was sent.
fn text(pieces: &[Piece]) -> String {
    let events = pieces.iter().flat_map(|p| match p {
        Piece::Event(e) => std::slice::from_ref(e),
        Piece::Frame(_, evs) => evs.as_slice(),
        Piece::Restart => &[],
    });
    events.filter_map(|e| if let Event::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect()
}

/// Every `data:` payload that parses as JSON.
fn payloads(bytes: &str) -> Vec<Value> {
    bytes.lines().filter_map(|l| l.strip_prefix("data: ")).filter_map(|d| serde_json::from_str(d).ok()).collect()
}

fn kinds(r: &RequestRecord) -> Vec<AttemptKind> {
    r.attempts.iter().map(|a| a.kind).collect()
}

fn words(n: usize) -> String {
    (0..n).map(|i| format!("w{i} ")).collect()
}

#[tokio::test]
async fn a_cut_answer_is_continued_without_a_seam() {
    for style in STYLES {
        let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", PREFILL))], &[("alpha", "main")], "").await;
        s.mock.push([cut_after_deltas(20), answer("tail.")]);
        let (pieces, bytes, r) = run(&s, style, "alpha/m1", "ak_test", false).await;
        assert_eq!(text(&pieces), format!("{}tail.", words(20)), "{style}: no repeated or missing delta");
        assert!(!bytes.contains(RESTART_NOTE), "{style}");
        let sent = s.mock.received()[1].json();
        let last = sent["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last, json!({"role": "assistant", "content": words(20).trim_end()}), "{style}: the prefill");
        assert_eq!(r.outcome, Outcome::Succeeded, "{style}");
        assert_eq!(r.break_handling, BreakHandling::Continued, "{style}");
        assert_eq!(kinds(&r).last(), Some(&AttemptKind::Continuation), "{style}");
        let u = r.usage.unwrap();
        assert_eq!((u.input, u.output), (Some(35), Some(23)), "{style}: the segments' usage adds up");
        if style == "anthropic-messages" {
            let starts = payloads(&bytes).iter().filter(|p| p["type"] == "content_block_start").count();
            assert_eq!(starts, 1, "the continuation merges into the open block: {bytes}");
        }
    }
}

#[tokio::test]
async fn by_default_a_cut_answer_restarts_after_a_note() {
    for style in STYLES {
        let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "main")], "").await;
        s.mock.push([cut_after_deltas(3), answer("again.")]);
        let (pieces, bytes, r) = run(&s, style, "alpha/m1", "ak_test", false).await;
        assert!(pieces.iter().any(|p| matches!(p, Piece::Restart)), "{style}");
        assert!(text(&pieces).ends_with("again."), "{style}");
        assert!(bytes.contains(RESTART_NOTE), "{style}: {bytes}");
        assert_eq!(r.outcome, Outcome::Succeeded, "{style}");
        assert_eq!(r.break_handling, BreakHandling::Restarted, "{style}");
        assert_eq!(kinds(&r).last(), Some(&AttemptKind::Restart), "{style}");
        let sent = s.mock.received()[1].json();
        assert_eq!(sent["messages"].as_array().unwrap().len(), 1, "{style}: the original request again");
        let p = payloads(&bytes);
        match style {
            "anthropic-messages" => {
                let idx: Vec<u64> = p
                    .iter()
                    .filter(|e| e["type"] == "content_block_start")
                    .map(|e| e["index"].as_u64().unwrap())
                    .collect();
                assert_eq!(idx, [0, 1, 2], "cut block, note block, new answer: {bytes}");
                let note = p.iter().find(|e| e["type"] == "content_block_delta" && e["index"] == 1).unwrap();
                assert_eq!(note["delta"]["text"], RESTART_NOTE);
            }
            "openai-responses" => {
                let seq: Vec<u64> = p.iter().filter_map(|e| e["sequence_number"].as_u64()).collect();
                assert!(seq.windows(2).all(|w| w[1] == w[0] + 1), "sequence numbers carry on: {seq:?}");
                let items: Vec<u64> = p
                    .iter()
                    .filter(|e| e["type"] == "response.output_item.added")
                    .map(|e| e["output_index"].as_u64().unwrap())
                    .collect();
                assert_eq!(items, [0, 1, 2], "{bytes}");
            }
            _ => assert!(
                bytes.contains(&format!("\\n\\n{RESTART_NOTE}\\n\\n")),
                "{style}: a delta with blank lines: {bytes}"
            ),
        }
    }
}

#[tokio::test]
async fn a_key_can_choose_the_error_event() {
    for style in STYLES {
        let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "main")], "").await;
        let mut keys = Keys::load(&s._dir.path().join(nullrouter_engine::keys::FILE)).unwrap();
        let key = keys.issue("strict", Some(BreakBehaviour::ErrorEvent)).unwrap().1.id.clone();
        keys.save().unwrap();
        s.engine.reload_blocking().unwrap();
        s.mock.push([cut_after_deltas(3), answer("unused")]);
        let (pieces, bytes, r) = run(&s, style, "alpha/m1", &key, false).await;
        assert!(matches!(pieces.last(), Some(Piece::Event(Event::Error(_)))), "{style}: {pieces:?}");
        assert!(!bytes.contains(RESTART_NOTE), "{style}");
        assert!(bytes.contains(&r.id), "{style}: the error names the record: {bytes}");
        assert_eq!(s.mock.received().len(), 1, "{style}");
        assert_eq!(r.outcome, Outcome::Failed, "{style}");
        assert!(matches!(r.break_handling, BreakHandling::ErrorEvent { .. }), "{style}");
        if style == "gemini" {
            let e = payloads(&bytes).into_iter().find(|p| p.get("error").is_some()).unwrap();
            assert_eq!(e["error"]["code"], 502, "google-genai reads the status from the code: {e}");
        }
    }
}

#[tokio::test]
async fn a_cut_tool_call_always_ends_with_the_error_event() {
    for style in STYLES {
        let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", PREFILL))], &[("alpha", "main")], "").await;
        s.mock.push([cut_in_tool_call(), answer("unused")]);
        let (pieces, _, r) = run(&s, style, "alpha/m1", "ak_test", false).await;
        assert!(matches!(pieces.last(), Some(Piece::Event(Event::Error(_)))), "{style}: {pieces:?}");
        assert_eq!(s.mock.received().len(), 1, "{style}");
        let BreakHandling::ErrorEvent { reason } = &r.break_handling else { panic!("{style}: {:?}", r.break_handling) };
        assert!(reason.contains("tool call"), "{style}: {reason}");
    }
}

#[tokio::test]
async fn a_cut_before_content_is_an_ordinary_retry() {
    for style in STYLES {
        let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "main")], "").await;
        let empty = spaced(Step::sse(&[chunk(json!({"role": "assistant"}), Value::Null)], true).cut_after(1));
        s.mock.push([empty, answer("fine.")]);
        let (pieces, bytes, r) = run(&s, style, "alpha/m1", "ak_test", false).await;
        assert!(!pieces.iter().any(|p| matches!(p, Piece::Restart)), "{style}");
        assert!(!bytes.contains(RESTART_NOTE), "{style}");
        assert_eq!(text(&pieces), "fine.", "{style}");
        assert_eq!(r.break_handling, BreakHandling::None, "{style}");
    }
}

#[tokio::test]
async fn an_excluded_continuation_falls_through_to_the_restart() {
    let unless = format!("{PREFILL}unless = [\"thinking_enabled\"]\n");
    let except = format!("{PREFILL}except_models = [\"m1\"]\n");
    // (continuation declaration, thinking on, first segment)
    let cases = [(unless.as_str(), true, cut_after_deltas(3)), (except.as_str(), false, cut_after_deltas(3))];
    for (decl, thinking, first) in cases {
        let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", decl))], &[("alpha", "main")], "").await;
        s.mock.push([first, answer("again.")]);
        let (_, _, r) = run(&s, "anthropic-messages", "alpha/m1", "ak_test", thinking).await;
        assert_eq!(r.break_handling, BreakHandling::Restarted, "{decl}");
    }
    // A partial answer holding thinking can't be carried as a prefill.
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", PREFILL))], &[("alpha", "main")], "").await;
    let thought = Step::sse(
        &[chunk(json!({"reasoning_content": "hmm"}), Value::Null), chunk(json!({"content": "So "}), Value::Null)],
        true,
    )
    .cut_after(2);
    let thought = spaced(thought);
    s.mock.push([thought, answer("again.")]);
    let (_, _, r) = run(&s, "openai-chat", "alpha/m1", "ak_test", false).await;
    assert_eq!(r.break_handling, BreakHandling::Restarted);
}

/// Spec 013: a stream cut after output and continued by attempt 2. The first attempt ends in
/// generation, the first token is the first attempt's, and the phases still add up.
#[tokio::test]
async fn a_cut_and_continued_answer_keeps_its_phases_consistent() {
    use nullrouter_engine::phases::{self, Phase, PhaseValue};

    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", PREFILL))], &[("alpha", "main")], "").await;
    s.mock.push([cut_after_deltas(5), answer("tail.")]);
    let (_, _, r) = run(&s, "openai-chat", "alpha/m1", "ak_test", false).await;
    assert_eq!(kinds(&r), [AttemptKind::Initial, AttemptKind::Continuation]);
    let all = phases::of(&r);
    assert_eq!(all[0].ended_in, Some(Phase::Generation), "the cut happened while the provider generated");
    assert_eq!(all[0].value(Phase::Delivery), PhaseValue::NotApplicable);
    assert_eq!(all[1].ended_in, None);
    let sum: f64 = all.iter().map(|p| p.sum()).sum();
    let total = r.total_ms.unwrap();
    assert!((sum - total).abs() <= 1.0, "phases add up to {sum:.2} ms, total is {total:.2} ms");
    // The request's first token is the first attempt's.
    let up_to: f64 = [Phase::RouterOverhead, Phase::Connect, Phase::Headers, Phase::FirstToken]
        .into_iter()
        .filter_map(|ph| all[0].value(ph).ms())
        .sum();
    assert!((up_to - r.ttft_ms.unwrap()).abs() <= 1.0, "{up_to} vs {:?}", r.ttft_ms);
}
