//! In-band errors (T121, US7-5, FR-024): a plugin declares an error inside a 200 body and
//! an error event in a stream. Both are classified with the declared status, class
//! `in_band`, and fall back to the next member.

mod common;

use common::{chat_body, chat_chunks, drain, ok, request, settled, setup, unified};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{Answer, Piece};
use zerorouter_engine::records::{AttemptOutcome, ErrorClass, RequestRecord};
use zerorouter_engine::testkit::{MockUpstream, Step};
use zerorouter_wire::ir::Event;

const RULES: &str = r#"[endpoints.text.errors]
body = [{ when = { path_present = "error" }, message = "error.message", status = "error.code" }]
stream = [{ event = "error", message = "error.message", status_map = { overloaded_error = 529 } }]"#;

fn plugins(m: &MockUpstream) -> Vec<(&'static str, String)> {
    vec![("flaky", common::chat_plugin(m, "flaky", RULES)), ("good", common::chat_plugin(m, "good", ""))]
}

/// The first attempt's provider, status and class.
fn first_failure(rec: &RequestRecord) -> (String, Option<u16>, ErrorClass) {
    let a = &rec.attempts[0];
    let Some(AttemptOutcome::Failed { status, class, .. }) = &a.outcome else { panic!("{a:#?}") };
    (a.provider.clone(), *status, *class)
}

#[tokio::test]
async fn an_error_in_a_200_body_is_classified_and_falls_back() {
    let s = setup(plugins, &[("flaky", "a"), ("good", "b")], &unified(&[("flaky", "m1"), ("good", "m1")])).await;
    s.mock.on("/flaky/", [Step::in_band_error(json!({"error": {"message": "quota gone", "code": 429}}))]);
    s.mock.on("/good/", [ok()]);
    let req = request(&s, "openai-chat", "u", chat_body("u", false), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let answer = s.engine.text(s.engine.snapshot(), req).await.unwrap_or_else(|f| panic!("{f:?}"));
    assert!(matches!(answer, Answer::Whole { .. }));
    let rec = settled(&s, &id).await;
    assert_eq!(first_failure(&rec), ("flaky".into(), Some(429), ErrorClass::InBand), "{:#?}", rec.attempts);
    assert_eq!(rec.attempts.last().unwrap().provider, "good");
}

#[tokio::test]
async fn a_stream_error_event_maps_its_type_and_falls_back() {
    let s = setup(plugins, &[("flaky", "a"), ("good", "b")], &unified(&[("flaky", "m1"), ("good", "m1")])).await;
    let error = json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}});
    s.mock.on("/flaky/", [Step::sse(&[(Some("error"), error)], false)]);
    s.mock.on("/good/", [chat_chunks()]);
    let req = request(&s, "openai-chat", "u", chat_body("u", true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Ok(Answer::Events { mut rx, .. }) = s.engine.text(s.engine.snapshot(), req).await else { panic!("no stream") };
    let pieces = drain(&mut rx).await;
    let text: String = pieces
        .iter()
        .flat_map(|p| match p {
            Piece::Event(e) => std::slice::from_ref(e),
            Piece::Frame(_, events) => &events[..],
            Piece::Restart => &[],
        })
        .filter_map(|e| match e {
            Event::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello", "the error frame never reached the client: {pieces:?}");
    let rec = settled(&s, &id).await;
    assert_eq!(first_failure(&rec), ("flaky".into(), Some(529), ErrorClass::InBand), "{:#?}", rec.attempts);
    assert_eq!(rec.attempts.last().unwrap().provider, "good");
}
