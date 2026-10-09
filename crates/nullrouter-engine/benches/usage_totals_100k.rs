//! Spec 010 SC-004 (T048): the time to total requests, tokens and cost for a period, on a
//! 100,000-record journal in 30 daily segments, five agents in rotation. Records are about 1 KB
//! each (a request's three lines), generated once into a temp home, as `keys_last_used_100k` does.
//!
//! Warm: the same window again in one process, with the day cache filled, as in a running server.
//! Cold: a fresh copy of the home for every run, so every segment in the window is read, as in
//! `nullrouter usage` with no server.
//!
//! Targets (spec 010 SC-004): warm under 50 ms, cold under 1 s. Baseline:
//! `specs/010-dashboard-summaries/bench-baseline.md`.
//!
//! `cargo bench -p nullrouter-engine --bench usage_totals_100k`

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use nullrouter_engine::clock::rfc3339;
use nullrouter_engine::journal::summary::{self, Window};
use ulid::Ulid;

/// 2026-09-01T00:00:00Z.
const START_MS: u64 = 1_788_220_800_000;
const DAY_MS: u64 = 86_400_000;

/// `count` requests spread evenly over `days` day segments; agent `ak_bench0` to `ak_bench4` in
/// rotation, served by anthropic with usage and a first-token time.
fn build(home: &Path, count: u64, days: u64) {
    fs::create_dir_all(home.join("records")).unwrap();
    let gap = (days * DAY_MS / count).max(1);
    let mut by_day: std::collections::BTreeMap<String, String> = Default::default();
    let pad = "x".repeat(380);
    for n in 0..count {
        let ms = START_MS + n * gap;
        let id = format!("rq_{}", Ulid::from_parts(ms, u128::from(n)));
        let at = rfc3339(UNIX_EPOCH + Duration::from_millis(ms));
        let text = by_day.entry(at[..10].to_owned()).or_default();
        let _ = writeln!(
            text,
            r#"{{"v":1,"t":"open","id":"{id}","arrived":"{at}","agent":"ak_bench{}","style":"anthropic-messages","op":"generate","type":"text","target":"sonnet"}}"#,
            n % 5
        );
        let _ = writeln!(
            text,
            r#"{{"v":1,"t":"attempt","id":"{id}","attempt":{{"n":1,"provider":"anthropic","account":"max","model":"claude-sonnet-4-5","kind":"initial","placement":{{"reason":"warm","rank":0}},"started":0.4,"ended":900.0,"outcome":{{"state":"ok"}},"dropped":[],"forced":[],"note":"{pad}"}}}}"#
        );
        let _ = writeln!(
            text,
            r#"{{"v":1,"t":"close","id":"{id}","outcome":"succeeded","served_by":{{"provider":"anthropic","account":"max","model":"claude-sonnet-4-5"}},"ttft_ms":120.0,"total_ms":900.0,"usage":{{"input":10,"output":20}},"break_handling":{{"kind":"none"}},"job":null}}"#
        );
    }
    for (day, text) in by_day {
        fs::write(home.join(format!("records/{day}.jsonl")), text).unwrap();
    }
}

/// A new home holding the journal's segments, so no segment of it is cached yet.
fn copy_home(from: &Path) -> tempfile::TempDir {
    let to = tempfile::tempdir().unwrap();
    fs::create_dir_all(to.path().join("records")).unwrap();
    for entry in fs::read_dir(from.join("records")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), to.path().join("records").join(entry.file_name())).unwrap();
    }
    to
}

/// The window ending `end_days` after the start, `back_days` long (`None`: from the beginning).
fn window(end_days: u64, back_days: Option<u64>) -> Window {
    let at = |days: u64| UNIX_EPOCH + Duration::from_millis(START_MS + days * DAY_MS);
    Window { from: back_days.map(|b| at(end_days - b)), to: at(end_days) }
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("usage_totals");
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), 100_000, 30);
    let home = dir.path().to_owned();
    let none = |_: &str, _: Option<&str>| None;
    let periods = [("today", window(30, Some(1))), ("30d", window(30, Some(30))), ("all", window(30, None))];

    for (name, w) in &periods {
        // The first read fills the day cache; every run after it reads the cache.
        assert!(summary::totals(&home, w, &none, false).requests > 0);
        group.sample_size(50);
        group.bench_function(format!("warm/{name}/100k_30_days"), |b| {
            b.iter(|| black_box(summary::totals(&home, w, &none, false)));
        });
        group.sample_size(10);
        group.bench_function(format!("cold/{name}/100k_30_days"), |b| {
            // The routine hands the home back, so it is removed outside the timing.
            b.iter_batched(
                || copy_home(&home),
                |copy| (black_box(summary::totals(copy.path(), w, &none, false)), copy),
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
