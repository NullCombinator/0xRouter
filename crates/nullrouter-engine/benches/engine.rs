//! R24 / SC-013 (T137): the time to first byte 0router adds, as the attempt loop against an
//! instant loopback mock. `direct` is the same streamed request sent straight to the mock
//! with reqwest; the engine benches minus `direct` are what 0router adds. Each bench also
//! prints the per-request median and p95 (target p95 ≤ 10 ms).
//!
//! `cargo bench -p nullrouter-engine --bench engine -- --save-baseline slice-003`

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::time::{Duration, Instant};

use common::{Setup, chat_body, chat_chunks, chat_plugin, request, setup};
use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_engine::attempt::Answer;
use serde_json::json;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

/// Prints the median and p95 of the individual request latencies.
fn report(name: &str, mut all: Vec<Duration>) {
    all.sort_unstable();
    let at = |q: f64| all[((all.len() - 1) as f64 * q).round() as usize];
    println!("{name}: per-request median {:?}, p95 {:?} over {} requests", at(0.5), at(0.95), all.len());
}

/// Sends one streamed request through the engine and returns the time until its first
/// piece, then drains the rest outside the measurement.
async fn first_piece(s: &Setup, client: &str, body: serde_json::Value) -> Duration {
    let st = s.engine.snapshot();
    let req = request(s, client, "mockco/m1", body, "ak_bench", CancellationToken::new());
    let start = Instant::now();
    let Ok(Answer::Events { mut rx, .. }) = s.engine.text(st, req).await else { panic!("expected a stream") };
    let first = rx.recv().await.expect("a first piece");
    let took = start.elapsed();
    black_box(first);
    while rx.recv().await.is_some() {}
    took
}

fn bench(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let s = rt.block_on(setup(|m| vec![("mockco", chat_plugin(m, "mockco", ""))], &[("mockco", "main")], ""));
    s.mock.respond(|_| chat_chunks());

    let mut group = c.benchmark_group("ttfb");
    group.measurement_time(Duration::from_secs(10));

    let direct = reqwest::Client::new();
    let url = s.mock.url("/mockco/chat/completions");
    let mut all = Vec::new();
    group.bench_function("direct", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let start = Instant::now();
                    let mut r = direct.post(&url).body(chat_body("m1", true).to_string()).send().await.unwrap();
                    black_box(r.chunk().await.unwrap());
                    let took = start.elapsed();
                    while r.chunk().await.unwrap().is_some() {}
                    all.push(took);
                    total += took;
                }
                total
            })
        })
    });
    report("ttfb/direct", std::mem::take(&mut all));

    let anthropic = json!({"model": "mockco/m1", "max_tokens": 64, "stream": true, "messages": [{"role": "user", "content": "hi"}]});
    for (name, client, body) in
        [("same_style", "openai-chat", chat_body("mockco/m1", true)), ("cross_style", "anthropic-messages", anthropic)]
    {
        group.bench_function(name, |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let took = first_piece(&s, client, body.clone()).await;
                        all.push(took);
                        total += took;
                    }
                    total
                })
            })
        });
        report(&format!("ttfb/{name}"), std::mem::take(&mut all));
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
