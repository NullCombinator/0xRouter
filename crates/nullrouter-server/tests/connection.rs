//! Timeouts and connection settings per provider and model (spec 013, US3): scenarios 1-4 and
//! 6, SC-006, SC-007, FR-031 and FR-032.
//!
//! The clock is real and the timeouts are scaled down (hundreds of ms for the spec's seconds
//! and minutes): the mock's delays go through real sockets, which a paused clock would race.
//!
//! Not covered here: that the header timeout counts connect time (R12). The mock can't delay a
//! client's connect, so it rests on the attempt's one `timeout` around the whole send.

mod common;

use std::time::Duration;

use axum::body::Bytes;
use common::{Server, chat_whole, server, server_with};
use nullrouter_engine::records::{
    AttemptOutcome, ErrorClass, RequestRecord, Source, SourceBy, SourceLevel, TimeoutHit, TimeoutKind,
};
use nullrouter_engine::testkit::Step;
use nullrouter_server::relay::REQUEST_ID;
use serde_json::{Value, json};

const OPERATOR_PROVIDER: Source = Source { by: SourceBy::Operator, level: SourceLevel::Provider };

/// `mockco` with models `fast` and `slow`, both on the mock's chat route.
fn duo(mock: &nullrouter_engine::testkit::MockUpstream) -> Vec<(&'static str, String)> {
    vec![(
        "duo",
        format!(
            "schema = 2\nid = \"duo\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"fast\"\n[[models]]\nid = \"slow\"\n",
            mock.url("/v1/chat/completions")
        ),
    )]
}

/// Rewrites `config.toml` and reloads, returning the reload's verdict.
async fn configure(s: &Server, extra: &str) -> Result<(), String> {
    std::fs::write(s.home().join("config.toml"), format!("allow_private_endpoints = true\n{extra}")).unwrap();
    s.engine.reload().await.map(|_| ()).map_err(|e| e.to_string())
}

fn ask(s: &Server, model: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", s.base))
        .bearer_auth(&s.key)
        .body(json!({"model": model, "stream": true, "messages": [{"role": "user", "content": "hi"}]}).to_string())
}

/// Sends, reads the answer to its end and returns the status and the finished record.
async fn run(s: &Server, req: reqwest::RequestBuilder) -> (u16, RequestRecord) {
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let id = r.headers()[REQUEST_ID].to_str().unwrap().to_owned();
    let _ = r.bytes().await;
    (status, finished(s, &id).await)
}

async fn finished(s: &Server, id: &str) -> RequestRecord {
    for _ in 0..400 {
        if let Some(rec) = s.engine.records.get(id)
            && rec.total_ms.is_some()
            && rec.attempts.last().is_some_and(|a| a.ended.is_some())
        {
            return rec;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("record {id} never finished");
}

fn data(v: Value) -> Bytes {
    Bytes::from(format!("data: {v}\n\n"))
}

fn delta(d: Value, finish: Value) -> Bytes {
    data(json!({"id": "up-1", "object": "chat.completion.chunk", "created": 1, "model": "m1",
                "choices": [{"index": 0, "delta": d, "finish_reason": finish}]}))
}

/// A stream that thinks in `n` deltas, then says "ok".
fn thinking(n: usize) -> Vec<Bytes> {
    let mut out: Vec<Bytes> =
        (0..n).map(|i| delta(json!({"role": "assistant", "reasoning_content": format!("t{i} ")}), Value::Null)).collect();
    out.push(delta(json!({"content": "ok"}), Value::Null));
    out.push(delta(json!({}), json!("stop")));
    out.push(Bytes::from_static(b"data: [DONE]\n\n"));
    out
}

fn phased(headers: u64, first: u64, every: u64, frames: Vec<Bytes>) -> Step {
    Step::Phased {
        headers_delay: Duration::from_millis(headers),
        first_frame_delay: Duration::from_millis(first),
        frames,
        every: Duration::from_millis(every),
    }
}

fn timeout_of(rec: &RequestRecord) -> Option<TimeoutHit> {
    rec.attempts[0].timing.as_ref().and_then(|t| t.timeout)
}

fn failed_as_timeout(rec: &RequestRecord) -> String {
    match &rec.attempts[0].outcome {
        Some(AttemptOutcome::Failed { class: ErrorClass::Timeout, reason, .. }) => reason.clone(),
        other => panic!("the attempt should have failed as a timeout: {other:?}"),
    }
}

#[tokio::test]
async fn an_operator_header_timeout_beats_the_default_and_removing_it_restores_the_default() {
    // A model that timed out rests, so the second half uses the provider's other model.
    let s = server_with(duo).await;
    configure(&s, "[provider.duo.connection]\nheader_timeout_ms = 200\n").await.unwrap();
    s.mock.push([Step::StallHeaders { hold: Duration::from_secs(3) }]);
    let (status, rec) = run(&s, ask(&s, "duo/fast")).await;
    assert_ne!(status, 200, "nothing else could serve it");
    let reason = failed_as_timeout(&rec);
    assert!(reason.contains("200 ms"), "{reason}");
    assert_eq!(
        timeout_of(&rec),
        Some(TimeoutHit { which: TimeoutKind::Headers, ms: 200, source: OPERATOR_PROVIDER })
    );

    // Scenario 2: the override is gone, so the same delay is fine under the built-in 60 s.
    configure(&s, "").await.unwrap();
    s.mock.push([phased(400, 0, 10, thinking(0))]);
    let (status, rec) = run(&s, ask(&s, "duo/slow")).await;
    assert_eq!(status, 200);
    assert_eq!(timeout_of(&rec), None);
}

#[tokio::test]
async fn a_model_has_its_own_first_token_timeout_over_its_providers() {
    let s = server_with(duo).await;
    configure(
        &s,
        "[provider.duo.connection]\nfirst_token_timeout_ms = 150\n\n[provider.duo.model.slow.connection]\nfirst_token_timeout_ms = 1500\n",
    )
    .await
    .unwrap();

    // Output comes 400 ms after the headers: too late for `fast`, in time for `slow`.
    s.mock.push([phased(0, 400, 10, thinking(0))]);
    // A stream is committed before its first output, so the client sees 200 and the failure
    // is on the record.
    let (_, rec) = run(&s, ask(&s, "duo/fast")).await;
    let reason = failed_as_timeout(&rec);
    assert!(reason.contains("no model output within 150 ms"), "{reason}");
    assert_eq!(
        timeout_of(&rec),
        Some(TimeoutHit { which: TimeoutKind::FirstToken, ms: 150, source: OPERATOR_PROVIDER })
    );

    s.mock.push([phased(0, 400, 10, thinking(0))]);
    let (status, rec) = run(&s, ask(&s, "duo/slow")).await;
    assert_eq!(status, 200, "the model's 1500 ms applies");
    assert_eq!(timeout_of(&rec), None);
}

#[tokio::test]
async fn a_timed_out_attempt_falls_over_to_the_next_provider() {
    let s = server_with(duo).await;
    // Two providers behind one unified model, both held to 200 ms: whichever is tried first
    // hangs, and the other answers.
    configure(
        &s,
        "[provider.duo.connection]\nheader_timeout_ms = 200\n\n[provider.mockco.connection]\nheader_timeout_ms = 200\n\n[[unified_model]]\nname = \"pair\"\nmembers = [{ provider = \"duo\", model = \"fast\" }, { provider = \"mockco\", model = \"m1\" }]\n",
    )
    .await
    .unwrap();
    s.mock.push([Step::StallHeaders { hold: Duration::from_secs(3) }, chat_whole()]);
    let (status, rec) = run(&s, ask(&s, "pair")).await;
    assert_eq!(status, 200, "the other provider served it");
    assert!(rec.attempts.len() >= 2, "{:?}", rec.attempts.len());
    failed_as_timeout(&rec);
}

/// SC-006, scaled: a thinking stream whose total time is several times every timeout, with gaps
/// below the stall timeout, is not cut.
#[tokio::test]
async fn a_thinking_stream_longer_than_every_timeout_is_never_cut() {
    let s = server().await;
    configure(
        &s,
        "[provider.mockco.connection]\nheader_timeout_ms = 400\nfirst_token_timeout_ms = 400\nstall_timeout_ms = 400\n",
    )
    .await
    .unwrap();
    // 14 thinking deltas 100 ms apart: about 1.5 s in all, nearly four times each timeout.
    s.mock.push([phased(0, 0, 100, thinking(14))]);
    let started = std::time::Instant::now();
    let (status, rec) = run(&s, ask(&s, "mockco/m1")).await;
    assert_eq!(status, 200);
    assert!(started.elapsed() > Duration::from_millis(1200), "{:?}", started.elapsed());
    assert_eq!(timeout_of(&rec), None);
    assert!(matches!(rec.attempts[0].outcome, Some(AttemptOutcome::Ok)), "{:?}", rec.attempts[0].outcome);
}

/// SC-007 and scenario 6: a request in flight keeps the timeout it started with.
#[tokio::test]
async fn a_reload_changes_the_next_request_and_not_the_one_in_flight() {
    let s = server().await;
    configure(&s, "[provider.mockco.connection]\nheader_timeout_ms = 3000\n").await.unwrap();
    s.mock.push([phased(700, 0, 10, thinking(0))]);
    let first = tokio::spawn(ask(&s, "mockco/m1").send());
    tokio::time::sleep(Duration::from_millis(150)).await;
    configure(&s, "[provider.mockco.connection]\nheader_timeout_ms = 100\n").await.unwrap();

    let r = first.await.unwrap().unwrap();
    assert_eq!(r.status().as_u16(), 200, "the in-flight request keeps its 3000 ms");
    r.bytes().await.unwrap();

    s.mock.push([phased(700, 0, 10, thinking(0))]);
    let (status, rec) = run(&s, ask(&s, "mockco/m1")).await;
    assert_ne!(status, 200);
    assert_eq!(
        timeout_of(&rec),
        Some(TimeoutHit { which: TimeoutKind::Headers, ms: 100, source: OPERATOR_PROVIDER })
    );
}

/// FR-032: a hand-edited bad value is refused with the field named, and the old settings stay.
#[tokio::test]
async fn an_invalid_timeout_is_refused_on_reload_and_the_previous_settings_stay() {
    let s = server().await;
    configure(&s, "[provider.mockco.connection]\nheader_timeout_ms = 250\n").await.unwrap();
    let err = configure(&s, "[provider.mockco.connection]\nheader_timeout_ms = 0\n").await.unwrap_err();
    assert!(err.contains("header_timeout_ms"), "{err}");
    assert_eq!(s.engine.snapshot().registry.settings("mockco").connection.header_timeout_ms, Some(250));

    s.mock.push([Step::StallHeaders { hold: Duration::from_secs(3) }]);
    let (_, rec) = run(&s, ask(&s, "mockco/m1")).await;
    assert_eq!(timeout_of(&rec).map(|t| t.ms), Some(250));
}

/// `connection.view`: every timeout with its source, and the models that differ.
#[tokio::test]
async fn the_connection_view_shows_each_timeout_with_its_source_and_the_models_that_differ() {
    let s = server_with(duo).await;
    configure(
        &s,
        "[provider.duo.connection]\nheader_timeout_ms = 10000\nfirst_token_timeout_ms = 5000\n\n[provider.duo.model.slow.connection]\nfirst_token_timeout_ms = 300000\n",
    )
    .await
    .unwrap();
    let v = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view", "provider": "duo"})).await;
    assert_eq!(v["ok"], true, "{v}");
    let p = &v["providers"][0];
    assert_eq!(p["id"], "duo");
    assert_eq!(p["timeouts"]["headers"], json!({"ms": 10000, "source": {"by": "operator", "level": "provider"}}));
    assert_eq!(p["timeouts"]["first_token"]["ms"], 5000);
    assert_eq!(p["timeouts"]["stall"], json!({"ms": 360000, "source": {"by": "built_in", "level": "default"}}));
    let models = p["models"].as_array().unwrap();
    assert_eq!(models.len(), 1, "only `slow` differs: {models:?}");
    assert_eq!(models[0]["id"], "slow");
    assert_eq!(
        models[0]["timeouts"]["first_token"],
        json!({"ms": 300000, "source": {"by": "operator", "level": "model"}})
    );

    // Removing the override shows the default again, with its source.
    configure(&s, "").await.unwrap();
    let v = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view", "provider": "duo"})).await;
    assert_eq!(v["providers"][0]["timeouts"]["first_token"], json!({"ms": null, "source": null}));
    assert_eq!(v["providers"][0]["models"], json!([]));

    let v = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view", "provider": "nope"})).await;
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("duo"), "{v}");
    let all = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view"})).await;
    assert!(all["providers"].as_array().unwrap().len() >= 2, "{all}");
}

/// `duo`, whose plugin declares that its endpoint doesn't speak HTTP/2.
fn no_h2(mock: &nullrouter_engine::testkit::MockUpstream) -> Vec<(&'static str, String)> {
    let (id, body) = duo(mock).remove(0);
    vec![(id, body.replace("wire = \"openai-chat\"\n", "wire = \"openai-chat\"\nhttp2 = false\n"))]
}

/// FR-033, FR-034: `reuse` and `http2` per provider, with the source that decided each.
#[tokio::test]
async fn reuse_and_http2_resolve_operator_over_plugin_over_default() {
    let s = server_with(no_h2).await;
    let view = |s: &Server| {
        let engine = s.engine.clone();
        async move {
            let v = nullrouter_server::operator::handle(&engine, &json!({"op": "connection.view", "provider": "duo"})).await;
            assert_eq!(v["ok"], true, "{v}");
            v["providers"][0].clone()
        }
    };

    configure(&s, "").await.unwrap();
    let p = view(&s).await;
    assert_eq!(p["http2"], json!({"value": "off", "source": {"by": "plugin", "level": "endpoint"}}), "{p}");
    assert_eq!(p["reuse"], json!({"value": true, "source": {"by": "built_in", "level": "default"}}), "{p}");

    // The operator's `on` beats the plugin; it asks to negotiate, not to force.
    configure(&s, "[provider.duo.connection]\nhttp2 = true\nreuse = false\n").await.unwrap();
    let p = view(&s).await;
    assert_eq!(p["http2"], json!({"value": "negotiate", "source": {"by": "operator", "level": "provider"}}), "{p}");
    assert_eq!(p["reuse"], json!({"value": false, "source": {"by": "operator", "level": "provider"}}), "{p}");

    configure(&s, "[provider.duo.connection]\nhttp2 = false\n").await.unwrap();
    let p = view(&s).await;
    assert_eq!(p["http2"], json!({"value": "off", "source": {"by": "operator", "level": "provider"}}), "{p}");

    // Both still serve a request.
    s.mock.respond(|_| chat_whole());
    let (status, rec) = run(&s, ask(&s, "duo/fast")).await;
    assert_eq!(status, 200);
    assert_eq!(rec.attempts[0].timing.as_ref().and_then(|t| t.http.as_deref()), Some("1.1"));
}

fn boom() -> Step {
    Step::json(503, json!({"error": {"message": "boom", "type": "server_error"}}))
}

fn kinds(rec: &RequestRecord) -> Vec<nullrouter_engine::records::AttemptKind> {
    rec.attempts.iter().map(|a| a.kind).collect()
}

/// `duo`, whose plugin declares two 503 retries 10 ms apart.
fn retrying(mock: &nullrouter_engine::testkit::MockUpstream) -> Vec<(&'static str, String)> {
    let (id, body) = duo(mock).remove(0);
    vec![(id, body.replace("wire = \"openai-chat\"\n", "wire = \"openai-chat\"\nretry = { \"503\" = { retries = 2, delay_ms = 10 } }\n"))]
}

/// US6 scenario 1: no same-account retries moves straight on.
#[tokio::test]
async fn zero_operator_retries_means_one_try() {
    let s = server_with(duo).await;
    configure(&s, "[provider.duo.retry]\nall = { retries = 0 }\n").await.unwrap();
    s.mock.respond(|_| boom());
    let (status, rec) = run(&s, ask(&s, "duo/fast")).await;
    assert_ne!(status, 200);
    assert_eq!(s.mock.received().len(), 1);
    assert_eq!(kinds(&rec), vec![nullrouter_engine::records::AttemptKind::Initial]);
}

/// US6 scenario 2: the operator's count and wait, with the wait recorded as its own phase.
#[tokio::test]
async fn the_operators_count_and_wait_apply_and_the_wait_is_recorded() {
    let s = server_with(duo).await;
    configure(&s, "[provider.duo.retry]\n\"503\" = { retries = 1, delay_ms = 400 }\n").await.unwrap();
    s.mock.respond(|_| boom());
    let (_, rec) = run(&s, ask(&s, "duo/fast")).await;
    use nullrouter_engine::records::AttemptKind::{Initial, SameAccountRetry};
    assert_eq!(kinds(&rec), vec![Initial, SameAccountRetry]);
    let wait = rec.attempts[1].timing.as_ref().and_then(|t| t.retry_wait_ms).expect("the wait is recorded");
    assert!((400.0..=420.0).contains(&wait), "within 5% of the configured wait: {wait}");
    assert!(rec.attempts[0].timing.as_ref().is_none_or(|t| t.retry_wait_ms.is_none()));
}

/// US6 scenario 3: the plugin's declaration applies until the operator sets the provider's own.
#[tokio::test]
async fn a_plugins_retries_apply_until_the_operator_sets_the_providers_own() {
    let s = server_with(retrying).await;
    configure(&s, "").await.unwrap();
    s.mock.respond(|_| boom());
    let (_, rec) = run(&s, ask(&s, "duo/fast")).await;
    assert_eq!(rec.attempts.len(), 3, "the plugin's two retries: {:?}", kinds(&rec));

    configure(&s, "[provider.duo.retry]\nall = { retries = 1, delay_ms = 5 }\n").await.unwrap();
    let (_, rec) = run(&s, ask(&s, "duo/slow")).await;
    assert_eq!(rec.attempts.len(), 2, "the operator's one: {:?}", kinds(&rec));

    // The view names who set what.
    let v = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view", "provider": "duo"})).await;
    assert_eq!(
        v["providers"][0]["retry"],
        json!([{"status": "all", "retries": 1, "delay_ms": 5, "source": {"by": "operator", "level": "provider"}}]),
        "{v}"
    );
    configure(&s, "").await.unwrap();
    let v = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view", "provider": "duo"})).await;
    assert_eq!(
        v["providers"][0]["retry"],
        json!([{"status": "503", "retries": 2, "delay_ms": 10, "source": {"by": "plugin", "level": "endpoint"}}]),
        "{v}"
    );
}

/// A value above the cap is refused on reload, with the field named, and the old settings stay.
#[tokio::test]
async fn retries_above_the_cap_are_refused_on_reload() {
    let s = server_with(duo).await;
    configure(&s, "[provider.duo.retry]\nall = { retries = 1 }\n").await.unwrap();
    let e = configure(&s, "[provider.duo.retry]\nall = { retries = 6 }\n").await.unwrap_err();
    assert!(e.contains("retries") && e.contains("0-5"), "{e}");
    let v = nullrouter_server::operator::handle(&s.engine, &json!({"op": "connection.view", "provider": "duo"})).await;
    assert_eq!(v["providers"][0]["retry"][0]["retries"], 1);
}
