//! Routing ignores latency in this slice (spec 013, research R16): SC-009 and FR-035.
//!
//! A unified model over two providers, one of them ten times slower, is placed exactly as when
//! both are equally fast. A second test reads the routing sources: none looks at `timing` or
//! `phases`.

mod common;

use std::time::Duration;

use common::*;
use nullrouter_engine::testkit::Step;
use tokio_util::sync::CancellationToken;

/// The chat chunks of [`chat_chunks`], sent after `headers_ms` of waiting.
fn after(headers_ms: u64) -> Step {
    let Step::Stream { frames, .. } = chat_chunks() else { unreachable!("chat_chunks streams") };
    Step::Phased {
        headers_delay: Duration::from_millis(headers_ms),
        first_frame_delay: Duration::ZERO,
        frames,
        every: Duration::ZERO,
    }
}

/// The providers that served eight requests of four agents, in order, when `beta` answers after
/// `beta_ms` and `alpha` after 30 ms.
async fn placements(beta_ms: u64) -> Vec<String> {
    let s = setup(
        |m| vec![("alpha", chat_plugin(m, "alpha", "")), ("beta", chat_plugin(m, "beta", ""))],
        &[("alpha", "main"), ("beta", "main")],
        &unified(&[("alpha", "m1"), ("beta", "m1")]),
    )
    .await;
    s.mock.respond(move |rx| {
        let bearer = rx.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
        after(if bearer.contains("-beta-") { beta_ms } else { 30 })
    });
    let mut served = Vec::new();
    for i in 0..8 {
        let agent = format!("ak_{}", i % 4);
        let req = request(&s, "openai-chat", "u", chat_body("u", true), &agent, CancellationToken::new());
        let id = req.id.clone();
        let nullrouter_engine::attempt::Answer::Events { mut rx, .. } =
            s.engine.text(s.engine.snapshot(), req).await.unwrap_or_else(|e| panic!("not served: {e:?}"))
        else {
            panic!("events")
        };
        drain(&mut rx).await;
        let rec = settled(&s, &id).await;
        served.push(rec.served_by.expect("served").provider);
    }
    served
}

#[tokio::test]
async fn a_provider_ten_times_slower_is_placed_as_if_it_were_not() {
    let equal = placements(30).await;
    let slow = placements(300).await;
    assert_eq!(equal, slow, "latency changed a placement");
}

#[test]
fn no_routing_code_reads_timing_or_phases() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.join("route.rs"), src.join("plan.rs")];
    files.extend(std::fs::read_dir(src.join("routing")).unwrap().map(|e| e.unwrap().path()));
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "rs")) {
        let text = std::fs::read_to_string(f).unwrap();
        for word in ["timing", "phases", "AttemptTiming", "latency"] {
            assert!(!text.contains(word), "{} mentions {word:?}: routing must not read measured latency", f.display());
        }
    }
}
