//! Spec 009 SC-009 (T072): the time to load each page, signed in, through the real dashboard
//! listener, on a home with 50 accounts, 20 unified models and 100,000 request records over 30
//! days (about 1 KB each). Each page is built and rendered from scratch on every load.
//!
//! Target: every page under 1 s. Baseline: `specs/009-dashboard/bench-baseline.md`.
//!
//! `cargo bench -p nullrouter-dashboard --bench pages`

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use nullrouter_dashboard::pages::Id;
use nullrouter_dashboard::spawn;
use nullrouter_engine::clock::rfc3339;
use nullrouter_engine::files::{DashboardToken, write_private};
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;
use nullrouter_registry::schema::DashboardSettings;
use reqwest::header::COOKIE;
use ulid::Ulid;

const TOKEN: &str = "nrd_bench0000000000000000000000000000000000000000";
/// 2026-09-01T00:00:00Z.
const START_MS: u64 = 1_788_220_800_000;
const ACCOUNTS: usize = 50;
const UNIFIED: usize = 20;
const RECORDS: u64 = 100_000;
const DAYS: u64 = 30;

/// 50 anthropic accounts, 20 unified models over them, and 100,000 records spread over 30 day
/// segments, each served by one of the accounts. Returns the newest record's id.
fn build(home: &Path) -> String {
    let mut accounts = String::new();
    for n in 0..ACCOUNTS {
        let _ = write!(accounts, "[[account]]\nprovider = \"anthropic\"\nname = \"acct-{n:02}\"\nsecret = \"sk-bench-{n:04}\"\norder = {n}\n\n");
    }
    write_private(&home.join(nullrouter_engine::accounts::FILE), &accounts).unwrap();
    let mut config = String::from("schema = 1\n");
    for n in 0..UNIFIED {
        let _ = write!(
            config,
            "\n[[unified_model]]\nname = \"unified-{n:02}\"\nmembers = [\n  {{ provider = \"anthropic\", model = \"claude-sonnet-4-20250514\" }},\n  {{ provider = \"anthropic\", model = \"claude-opus-4-20250514\" }},\n]\n"
        );
    }
    fs::write(home.join("config.toml"), config).unwrap();
    DashboardToken { digest: Some(DashboardToken::digest_of(TOKEN)), issued: Some("2026-10-07T08:00:00Z".into()) }
        .save(home)
        .unwrap();

    fs::create_dir_all(home.join("records")).unwrap();
    let gap = DAYS * 86_400_000 / RECORDS;
    let mut by_day: std::collections::BTreeMap<String, String> = Default::default();
    let mut newest = String::new();
    let pad = "x".repeat(380);
    for n in 0..RECORDS {
        let ms = START_MS + n * gap;
        let id = format!("rq_{}", Ulid::from_parts(ms, u128::from(n)));
        let at = rfc3339(UNIX_EPOCH + Duration::from_millis(ms));
        let account = format!("acct-{:02}", n as usize % ACCOUNTS);
        let target = format!("unified-{:02}", n as usize % UNIFIED);
        let text = by_day.entry(at[..10].to_owned()).or_default();
        let _ = writeln!(
            text,
            r#"{{"v":1,"t":"open","id":"{id}","arrived":"{at}","agent":"ak_bench{}","style":"anthropic-messages","op":"generate","type":"text","target":"{target}"}}"#,
            n % 5
        );
        let _ = writeln!(
            text,
            r#"{{"v":1,"t":"attempt","id":"{id}","attempt":{{"n":1,"provider":"anthropic","account":"{account}","model":"claude-sonnet-4-20250514","kind":"initial","placement":{{"reason":"warm","rank":0}},"started":0.4,"ended":900.0,"outcome":{{"state":"ok"}},"dropped":[],"forced":[],"note":"{pad}"}}}}"#
        );
        let _ = writeln!(
            text,
            r#"{{"v":1,"t":"close","id":"{id}","outcome":"succeeded","served_by":{{"provider":"anthropic","account":"{account}","model":"claude-sonnet-4-20250514"}},"ttft_ms":120.0,"total_ms":900.0,"usage":{{"input":10,"output":20}},"break_handling":{{"kind":"none"}},"job":null}}"#
        );
        newest = id;
    }
    for (day, text) in by_day {
        fs::write(home.join(format!("records/{day}.jsonl")), text).unwrap();
    }
    newest
}

fn bench(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let newest = build(dir.path());
    let (engine, _) = Engine::open(OperatorHome::new(dir.path())).expect("the bench home opens");
    let engine = Arc::new(engine);
    let settings = DashboardSettings { enabled: true, listen: "127.0.0.1:0".into() };
    let handle = rt.block_on(spawn(engine, &settings, "nullrouter bench", std::future::pending::<()>()));
    let addr = handle.addr().expect("the dashboard bound a loopback port");
    let http = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(30)).build().unwrap();

    let mut paths: Vec<String> = Id::ALL.iter().map(|id| id.path().to_owned()).collect();
    paths.extend(["/providers/anthropic".into(), format!("/usage/records/{newest}"), "/quota?notices".into()]);

    let mut group = c.benchmark_group("pages");
    group.sample_size(10);
    for path in &paths {
        let load = || async {
            let r = http.get(format!("http://{addr}{path}")).header(COOKIE, format!("nr_dashboard={TOKEN}")).send().await.unwrap();
            assert!(r.status().is_success(), "{path}: {}", r.status());
            r.bytes().await.unwrap()
        };
        group.bench_function(path.as_str(), |b| b.iter(|| black_box(rt.block_on(load()))));
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
