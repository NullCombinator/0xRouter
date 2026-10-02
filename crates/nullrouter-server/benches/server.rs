//! R24 (T137): the access-key check, route matching, and an end-to-end loopback request
//! through the running server to an instant mock. The end-to-end benches also print the
//! per-request median and p95.
//!
//! `cargo bench -p nullrouter-server --bench server -- --save-baseline slice-003`

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, HeaderValue, Method};
use common::{chat_stream, chat_whole, server};
use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_server::auth;
use nullrouter_server::router::RouteTable;
use serde_json::json;
use tokio::runtime::Runtime;

/// Prints the median and p95 of the individual request latencies.
fn report(name: &str, mut all: Vec<Duration>) {
    all.sort_unstable();
    let at = |q: f64| all[((all.len() - 1) as f64 * q).round() as usize];
    println!("{name}: per-request median {:?}, p95 {:?} over {} requests", at(0.5), at(0.95), all.len());
}

fn bench(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let s = rt.block_on(server());
    let st = s.engine.snapshot();

    let carriers = &st.registry.styles().find(|f| f.id == "openai-chat").unwrap().access_key.carriers;
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_str(&format!("Bearer {}", s.key)).unwrap());
    let mut unknown = HeaderMap::new();
    unknown.insert("authorization", HeaderValue::from_static("Bearer zr_not_a_key"));
    let mut group = c.benchmark_group("auth");
    group.bench_function("valid", |b| {
        b.iter(|| black_box(auth::check(&st.keys, carriers, black_box(&headers), None).is_ok()))
    });
    group.bench_function("unknown", |b| {
        b.iter(|| black_box(auth::check(&st.keys, carriers, black_box(&unknown), None).is_err()))
    });
    group.finish();

    let table = RouteTable::build(st.registry.styles()).unwrap();
    let mut group = c.benchmark_group("route");
    for (name, path) in [
        ("chat", "/v1/chat/completions"),
        ("messages", "/v1/messages"),
        ("gemini", "/v1beta/models/gemini-2.5-pro:streamGenerateContent"),
        ("none", "/v1/nothing/here"),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| black_box(table.candidates(&Method::POST, black_box(path), &headers).len()))
        });
    }
    group.finish();

    s.mock.respond(|r| if r.json()["stream"] == true { chat_stream() } else { chat_whole() });
    let client = reqwest::Client::new();
    let url = format!("{}/v1/chat/completions", s.base);
    let mut group = c.benchmark_group("e2e");
    group.measurement_time(Duration::from_secs(10));
    for (name, stream) in [("whole", false), ("stream_ttfb", true)] {
        let body = json!({"model": "mockco/m1", "stream": stream, "messages": [{"role": "user", "content": "hi"}]})
            .to_string();
        let mut all = Vec::new();
        group.bench_function(name, |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let start = Instant::now();
                        let mut r = client.post(&url).bearer_auth(&s.key).body(body.clone()).send().await.unwrap();
                        assert_eq!(r.status(), 200);
                        // Whole: the full body; stream: the first chunk, the rest drained unmeasured.
                        let took = if stream {
                            black_box(r.chunk().await.unwrap());
                            let took = start.elapsed();
                            while r.chunk().await.unwrap().is_some() {}
                            took
                        } else {
                            black_box(r.bytes().await.unwrap());
                            start.elapsed()
                        };
                        all.push(took);
                        total += took;
                    }
                    total
                })
            })
        });
        report(&format!("e2e/{name}"), all);
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
