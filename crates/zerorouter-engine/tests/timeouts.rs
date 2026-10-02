//! Timeouts (T066, research R10): no headers in time is a retried 502, a body that goes
//! quiet is a break, and the environment defaults parse like 9router's `envMs`.

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{self, Answer, Piece};
use zerorouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass, Outcome};
use zerorouter_engine::testkit::Step;
use zerorouter_engine::upstream::{env_ms, parse_ms};
use zerorouter_wire::ir::Event;

fn class(o: &Option<AttemptOutcome>) -> Option<ErrorClass> {
    match o {
        Some(AttemptOutcome::Failed { class, .. }) => Some(*class),
        _ => None,
    }
}

fn frame(delta: &str) -> axum::body::Bytes {
    format!("data: {}\n\n", json!({"id": "c1", "object": "chat.completion.chunk", "model": "m1", "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}]})).into()
}

#[tokio::test]
async fn no_headers_in_time_counts_as_a_502_and_is_retried() {
    let s = setup(
        |m| {
            vec![(
                "alpha",
                chat_plugin(m, "alpha", "timeout_ms = 200\nretry = { 502 = { retries = 1, delay_ms = 10 } }"),
            )]
        },
        &[("alpha", "main")],
        "",
    )
    .await;
    s.mock.push([Step::StallHeaders { hold: Duration::from_secs(5) }, ok()]);
    let (id, res) = send(&s, "alpha/m1").await;
    assert!(res.is_ok());
    let r = s.engine.records.get(&id).unwrap();
    assert_eq!(class(&r.attempts[0].outcome), Some(ErrorClass::Timeout));
    assert_eq!(r.attempts[1].kind, AttemptKind::SameAccountRetry, "the 502 budget applied");
    let Some(AttemptOutcome::Failed { reason, .. }) = &r.attempts[0].outcome else { unreachable!() };
    assert!(reason.contains("200 ms"), "{reason}");
}

#[tokio::test]
async fn a_quiet_stream_before_output_is_an_ordinary_retry() {
    let s = setup(
        |m| {
            vec![(
                "alpha",
                chat_plugin(m, "alpha", "stall_timeout_ms = 200\nretry = { 502 = { retries = 1, delay_ms = 10 } }"),
            )]
        },
        &[("alpha", "main")],
        "",
    )
    .await;
    s.mock.push([Step::StallAfter { frames: vec![], hold: Duration::from_secs(5) }, chat_chunks()]);
    let req = request(&s, "openai-chat", "alpha/m1", chat_body("alpha/m1", true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    let pieces = drain(&mut rx).await;
    assert!(!pieces.iter().any(|p| matches!(p, Piece::Event(Event::Error(_)))), "the client never hears of it");
    let r = settled(&s, &id).await;
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert_eq!(class(&r.attempts[0].outcome), Some(ErrorClass::Stall));
}

#[tokio::test]
async fn a_quiet_stream_after_output_is_a_break() {
    let config = "[pipeline]\nbreak_behaviour = \"error_event\"\n";
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", "stall_timeout_ms = 200"))], &[("alpha", "main")], config)
        .await;
    s.mock.push([Step::StallAfter { frames: vec![frame("Hel")], hold: Duration::from_secs(5) }]);
    let req = request(&s, "openai-chat", "alpha/m1", chat_body("alpha/m1", true), "ak_test", CancellationToken::new());
    let id = req.id.clone();
    let Answer::Events { mut rx, .. } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("events")
    };
    let pieces = drain(&mut rx).await;
    assert!(matches!(pieces.last(), Some(Piece::Event(Event::Error(_)))), "{pieces:?}");
    let r = settled(&s, &id).await;
    assert_eq!(r.outcome, Outcome::Failed);
    assert_eq!(class(&r.attempts[0].outcome), Some(ErrorClass::Stall));
    assert_eq!(s.mock.received().len(), 1);
}

#[tokio::test]
async fn a_forced_stream_that_goes_quiet_fails_the_whole_answer() {
    let s = setup(
        |m| {
            let forced = chat_plugin(m, "forced", "force_stream = true\nstall_timeout_ms = 200");
            vec![("forced", forced)]
        },
        &[("forced", "main")],
        "",
    )
    .await;
    s.mock.push([Step::StallAfter { frames: vec![frame("Hel")], hold: Duration::from_secs(5) }]);
    let body = chat_body("forced/m1", false);
    let req = request(&s, "openai-chat", "forced/m1", body.clone(), "ak_test", CancellationToken::new());
    let client = req.client.clone();
    let id = req.id.clone();
    let Answer::Events { rx, forced: true } = s.engine.text(s.engine.snapshot(), req).await.unwrap() else {
        panic!("forced events")
    };
    let err = attempt::collect(&client, &body, rx).await.expect_err("a break, not half an answer");
    assert!(err.message.contains(&id), "{}", err.message);
    assert_eq!(class(&settled(&s, &id).await.attempts[0].outcome), Some(ErrorClass::Stall));
}

#[test]
fn env_ms_takes_a_positive_integer_else_the_default() {
    assert_eq!(parse_ms("1500", 7), 1500);
    assert_eq!(parse_ms("  250ms", 7), 250, "leading digits, like parseInt");
    for bad in ["0", "-5", "soon", ""] {
        assert_eq!(parse_ms(bad, 7), 7, "{bad:?}");
    }
    assert_eq!(env_ms("ZR_TEST_TIMEOUTS_UNSET", 7), 7);
}
