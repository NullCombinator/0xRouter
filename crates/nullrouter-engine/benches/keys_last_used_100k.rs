//! Spec 009 R9 (FR-029a): the time to find when each agent key last arrived, on a 100,000-record
//! journal in 30 daily segments, five agents in rotation. Records are about 1 KB each (a request's
//! three lines), generated once into a temp home, as `records_page` does.
//!
//! Warm: every key is used, and the segment index is already cached, as in a running server.
//! Cold: a fresh copy of the home for every run (so nothing is cached), with one key in
//! `keys.toml` that no record names, so every segment is read, as in `keys list` with no server.
//!
//! Targets (research R9): warm at most 5 ms, cold at most 1 s.
//!
//! `cargo bench -p nullrouter-engine --bench keys_last_used_100k`

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use nullrouter_engine::clock::rfc3339;
use nullrouter_engine::journal::records;
use ulid::Ulid;

/// 2026-09-01T00:00:00Z.
const START_MS: u64 = 1_788_220_800_000;

/// `count` requests spread evenly over `days` day segments; agent `ak_bench0` to `ak_bench4` in
/// rotation.
fn build(home: &Path, count: u64, days: u64) {
    fs::create_dir_all(home.join("records")).unwrap();
    let gap = days * 86_400_000 / count;
    let mut by_day: std::collections::BTreeMap<String, String> = Default::default();
    for n in 0..count {
        let ms = START_MS + n * gap;
        let id = format!("rq_{}", Ulid::from_parts(ms, u128::from(n)));
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
}

/// `keys.toml` with the five agents the journal names, and one more that it never does.
fn write_keys(home: &Path, with_unused: bool) {
    let mut text = String::from("schema = 1\n");
    let ids = (0..5).map(|n| format!("ak_bench{n}")).chain(with_unused.then(|| "ak_never".to_owned()));
    for id in ids {
        let _ = write!(
            text,
            "\n[[key]]\nid = \"{id}\"\nname = \"{id}\"\ndigest = \"sha256:00\"\nlast4 = \"0000\"\ncreated = \"2026-09-01T00:00:00Z\"\n"
        );
    }
    let mut options = fs::OpenOptions::new();
    let mut file = options.write(true).create(true).truncate(true).mode(0o600).open(home.join("keys.toml")).unwrap();
    std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
}

/// A new home holding the journal's segments, so no segment of it is indexed yet.
fn copy_home(from: &Path) -> tempfile::TempDir {
    let to = tempfile::tempdir().unwrap();
    fs::create_dir_all(to.path().join("records")).unwrap();
    for entry in fs::read_dir(from.join("records")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), to.path().join("records").join(entry.file_name())).unwrap();
    }
    write_keys(to.path(), true);
    to
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("keys_last_used");
    let dir = tempfile::tempdir().unwrap();
    build(dir.path(), 100_000, 30);

    write_keys(dir.path(), false);
    let home = dir.path().to_owned();
    // The first read indexes the newest segment; every run after it reads the cached index.
    assert_eq!(records::last_used(&home).len(), 5);
    group.sample_size(50);
    group.bench_function("warm/100k_30_days", |b| b.iter(|| black_box(records::last_used(&home))));

    group.sample_size(10);
    group.bench_function("cold/100k_30_days", |b| {
        // The routine hands the home back, so it is removed outside the timing.
        b.iter_batched(
            || copy_home(&home),
            |copy| (black_box(records::last_used(copy.path())), copy),
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
