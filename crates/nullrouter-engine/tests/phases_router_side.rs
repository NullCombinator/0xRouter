//! Time spent on 0router's side shows as router overhead (spec 013, SC-001, T014).
//!
//! The server stamps a request's arrival; here the test backdates it, as if the request had
//! waited 300 ms inside 0router (decoding, the journal's open line, the plan) before its first
//! attempt began.

mod common;

use std::time::{Duration, Instant};

use common::*;
use nullrouter_engine::phases::{self, Phase, PhaseValue};
use tokio_util::sync::CancellationToken;

const WAITED: Duration = Duration::from_millis(300);

#[tokio::test]
async fn time_before_the_first_attempt_is_router_overhead_and_the_phases_add_up() {
    let s = setup(|m| vec![("alpha", chat_plugin(m, "alpha", ""))], &[("alpha", "main")], "").await;
    s.mock.push([ok()]);
    let mut req = request(&s, "openai-chat", "alpha/m1", chat_body("alpha/m1", false), "ak_test", CancellationToken::new());
    req.arrived = Instant::now().checked_sub(WAITED).expect("the clock is past 300 ms");
    let id = req.id.clone();
    s.engine.text(s.engine.snapshot(), req).await.expect("served");

    let r = settled(&s, &id).await;
    let first = &r.attempts[0];
    let p = &phases::of(&r)[0];
    let PhaseValue::Ms(overhead) = p.value(Phase::RouterOverhead) else { panic!("{:?}", p.value(Phase::RouterOverhead)) };
    let want = WAITED.as_secs_f64() * 1e3;
    assert!((overhead - want).abs() <= (want * 0.05).max(10.0) + 20.0, "router overhead {overhead:.1} ms, expected about {want}");
    assert!(first.started >= want, "the attempt began after the wait: {}", first.started);
    let sum: f64 = phases::of(&r).iter().map(|p| p.sum()).sum();
    assert!((sum - r.total_ms.unwrap()).abs() <= 1.0, "{sum} vs {:?}", r.total_ms);
    // The longest phase of the request is the one on 0router's side.
    assert_eq!(phases::slowest(&phases::of(&r)).map(|(ph, _)| ph), Some(Phase::RouterOverhead));
}
