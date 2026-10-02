//! Classification parity with 9router (T062, research R6): every case in
//! `tests/fixtures/9router/classify/cases.json`, generated from `checkFallbackError`.

use nullrouter_engine::classify::{self, backoff_ms};
use serde_json::Value;

fn oracle() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/9router/classify/cases.json");
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("run tools/gen-bundled/generate.mjs")).unwrap();
    v["data"].clone()
}

#[test]
fn every_oracle_case_gets_9routers_verdict_and_cooldown() {
    let data = oracle();
    let cases = data["cases"].as_array().unwrap();
    assert!(cases.len() > 1000);
    let mut bad = Vec::new();
    for c in cases {
        let status = c["status"].as_u64().unwrap() as u16;
        let text = c["text"].as_str().unwrap();
        let level = c["level"].as_u64().unwrap() as u8;
        let v = classify::text(status, text);
        let (ms, next) = v.cooldown_ms(level);
        let want_level = c["new_level"].as_u64().map_or(level, |l| l as u8);
        let got = (v.fallback, ms, next);
        let want = (c["fallback"].as_bool().unwrap(), c["cooldown_ms"].as_u64().unwrap(), want_level);
        if got != want {
            bad.push(format!("[{status}] {text:?} level {level}: got {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{} of {} cases differ:\n{}", bad.len(), cases.len(), bad.join("\n"));
}

#[test]
fn backoff_progression_matches() {
    for b in oracle()["backoff"].as_array().unwrap() {
        let level = b["level"].as_u64().unwrap() as u8;
        assert_eq!(backoff_ms(level), b["cooldown_ms"].as_u64().unwrap(), "level {level}");
    }
    assert_eq!(backoff_ms(200), 300_000);
}

#[test]
fn the_body_is_matched_with_its_status_prefix() {
    // JSON keys match: `overloaded_error` is a rate-limit wording, `rate_limit_error` isn't.
    assert!(classify::upstream(529, r#"{"error":{"type":"overloaded_error","message":"Overloaded"}}"#).fallback);
    assert!(!classify::upstream(400, r#"{"error":{"type":"rate_limit_error","message":"slow"}}"#).fallback);
}
