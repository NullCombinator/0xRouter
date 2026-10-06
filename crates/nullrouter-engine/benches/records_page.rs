//! Spec 008 R7 (SC-004): the time to read a page of request records, on 1k records, and on 100k in
//! 30 daily segments and in one segment, newest page and a page 50,000 records back. Records are
//! about 1 KB each (a request's three lines), generated once per case into a temp home.
//!
//! Targets (research R7): the newest page of 50 on 100k records in either layout is at most 20 ms and
//! within 2x of the 1k case; a page 50,000 back is at most 1 s.
//!
//! `cargo bench -p nullrouter-engine --bench records_page`

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_engine::clock::rfc3339;
use nullrouter_engine::journal::records::{self, Filter};
use ulid::Ulid;

/// 2026-09-01T00:00:00Z.
const START_MS: u64 = 1_788_220_800_000;

/// `count` requests spread evenly over `days` day segments, newest on the last day. Returns the id
/// `back` records from the newest.
fn build(home: &Path, count: u64, days: u64, back: u64) -> String {
    fs::create_dir_all(home.join("records")).unwrap();
    let span = days * 86_400_000;
    let gap = span / count;
    let mut by_day: std::collections::BTreeMap<String, String> = Default::default();
    let mut cursor = String::new();
    for n in 0..count {
        let ms = START_MS + n * gap;
        let id = format!("rq_{}", Ulid::from_parts(ms, u128::from(n)));
        if n == count - 1 - back.min(count - 1) {
            cursor = id.clone();
        }
        let at = rfc3339(UNIX_EPOCH + Duration::from_millis(ms));
        let text = by_day.entry(at[..10].to_owned()).or_default();
        // About 1 KB: the attempt carries a long reason, as a failed one would.
        let pad = "x".repeat(380);
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
            r#"{{"v":1,"t":"close","id":"{id}","outcome":"succeeded","served_by":{{"provider":"anthropic","account":"max","model":"claude-sonnet-4-5"}},"ttft_ms":120.0,"total_ms":900.0,"usage":null,"break_handling":{{"kind":"none"}},"job":null}}"#
        );
    }
    for (day, text) in by_day {
        fs::write(home.join(format!("records/{day}.jsonl")), text).unwrap();
    }
    cursor
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("records_page");
    group.sample_size(20);
    let page = |home: &Path, before: Option<String>| {
        records::read(home, &Filter { limit: Some(50), before, ..Filter::default() })
    };
    for (name, count, days) in [("1k", 1_000, 1), ("100k_30_days", 100_000, 30), ("100k_one_day", 100_000, 1)] {
        let dir = tempfile::tempdir().unwrap();
        let cursor = build(dir.path(), count, days, 50_000.min(count - 1));
        let home = dir.path().to_owned();
        group.bench_function(format!("newest_50/{name}"), |b| b.iter(|| black_box(page(&home, None))));
        if count > 1_000 {
            group.sample_size(10);
            group.bench_function(format!("after_50000_back/{name}"), |b| {
                b.iter(|| black_box(page(&home, Some(cursor.clone()))))
            });
            group.sample_size(20);
        }
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
