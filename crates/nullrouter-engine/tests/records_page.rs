//! Paging back through the request records (spec 008, US2, SC-005): pages joined equal the full
//! listing, with and without filters and with unfinished requests, on a journal of several days
//! and on one big day; the `open` lines of nearby requests may be written out of id order; and a
//! cursor must name a record.

use std::fs;
use std::path::Path;

use nullrouter_engine::clock::parse_rfc3339;
use nullrouter_engine::journal::records::{self, Filter};
use serde_json::{Value, json};
use ulid::Ulid;

const DAY_MS: u64 = 86_400_000;
/// 2026-10-01T00:00:00Z.
const START_MS: u64 = 1_790_812_800_000;

fn rfc(ms: u64) -> String {
    nullrouter_engine::clock::rfc3339(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms))
}

fn id(ms: u64, n: u64) -> String {
    format!("rq_{}", Ulid::from_parts(ms, u128::from(n)))
}

/// The lines of request `n` arriving at `ms`: by agent `a0`..`a2`, unfinished when `n % 7 == 0`.
fn lines(ms: u64, n: u64) -> String {
    let (i, at, agent) = (id(ms, n), rfc(ms), format!("a{}", n % 3));
    let mut out = String::new();
    let mut put = |v: Value| out += &(v.to_string() + "\n");
    put(
        json!({"v":1,"t":"open","id":i,"arrived":at,"agent":agent,"style":"anthropic-messages","op":"generate","type":"text","target":"sonnet"}),
    );
    put(
        json!({"v":1,"t":"attempt","id":i,"attempt":{"n":1,"provider":"anthropic","account":"max","model":"m","kind":"initial",
        "placement":{"reason":"warm","rank":0},"started":0.4,"ended":900.0,"outcome":{"state":"ok"},"dropped":[],"forced":[]}}),
    );
    if n % 7 != 0 {
        put(
            json!({"v":1,"t":"close","id":i,"outcome":"succeeded","served_by":{"provider":"anthropic","account":"max","model":"m"},
            "ttft_ms":120.0,"total_ms":900.0,"usage":null,"break_handling":{"kind":"none"},"job":null}),
        );
    }
    out
}

/// `count` requests, `gap_ms` apart from the start of 2026-10-01, written to day segments. Every
/// `swap`th pair is written in the other order, so its `open` lines are out of id order by
/// `gap_ms` (under the 60 s margin).
fn journal(home: &Path, count: u64, gap_ms: u64, swap: u64) {
    fs::create_dir_all(home.join("records")).unwrap();
    let mut n = 0;
    let mut by_day: std::collections::BTreeMap<String, String> = Default::default();
    while n < count {
        let at = |k: u64| START_MS + k * gap_ms;
        let mut batch = vec![(at(n), n)];
        if swap > 0 && n % swap == 0 && n + 1 < count {
            batch.insert(0, (at(n + 1), n + 1));
            n += 1;
        }
        for (ms, k) in batch {
            by_day.entry(rfc(ms)[..10].to_owned()).or_default().push_str(&lines(ms, k));
        }
        n += 1;
    }
    for (day, text) in by_day {
        fs::write(home.join(format!("records/{day}.jsonl")), text).unwrap();
    }
}

fn pages(home: &Path, filter: &Filter, limit: usize) -> Vec<Value> {
    let mut all = Vec::new();
    let mut before = None;
    loop {
        let page = records::read(home, &Filter { limit: Some(limit), before: before.clone(), ..filter.clone() });
        assert!(page.len() <= limit);
        let Some(last) = page.last() else { break };
        before = last["id"].as_str().map(str::to_owned);
        all.extend(page);
    }
    all
}

fn check(home: &Path, limits: &[usize]) {
    let filters = [
        Filter::default(),
        Filter { agent: Some("a1".into()), ..Filter::default() },
        Filter { reason: Some("warm".into()), account: Some("anthropic/max".into()), ..Filter::default() },
        Filter { since: parse_rfc3339("2026-10-01T03:00:00Z"), ..Filter::default() },
    ];
    for filter in filters {
        let full = records::read(home, &filter);
        assert!(!full.is_empty());
        for &limit in limits.iter().filter(|l| **l > 1 || filter.agent.is_none() && filter.since.is_none()) {
            let got = pages(home, &filter, limit);
            let ids = |v: &[Value]| v.iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
            assert_eq!(ids(&got), ids(&full), "limit {limit}, filter {filter:?}");
            assert_eq!(got, full, "the records are the same, in-flight ones included");
        }
    }
}

#[test]
fn pages_join_to_the_full_listing_over_several_days() {
    let home = tempfile::tempdir().unwrap();
    // 400 requests 10 minutes apart: about three days.
    journal(home.path(), 400, 600_000, 0);
    assert!(fs::read_dir(home.path().join("records")).unwrap().count() >= 3);
    check(home.path(), &[1, 7, 50]);
}

#[test]
fn pages_join_to_the_full_listing_in_one_big_day() {
    let home = tempfile::tempdir().unwrap();
    // 700 requests 30 s apart: one day, several blocks of 64 KiB.
    journal(home.path(), 700, 30_000, 0);
    let segment = fs::metadata(home.path().join("records/2026-10-01.jsonl")).unwrap().len();
    assert!(segment > 4 * 64 * 1024, "{segment} bytes");
    check(home.path(), &[7, 50]);
}

#[test]
fn open_lines_written_out_of_id_order_within_the_margin_still_page_correctly() {
    let home = tempfile::tempdir().unwrap();
    // Every third pair's two requests are written in the wrong order, 50 s apart in id time.
    journal(home.path(), 600, 50_000, 3);
    check(home.path(), &[7, 50]);
}

#[test]
fn a_cursor_on_a_day_boundary_and_one_that_is_no_record() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("records")).unwrap();
    // The last request of 2026-10-01 and the first of 2026-10-02.
    let (late, early) = (START_MS + DAY_MS - 1_000, START_MS + DAY_MS + 1_000);
    fs::write(home.path().join("records/2026-10-01.jsonl"), lines(late - 60_000, 1) + &lines(late, 2)).unwrap();
    fs::write(home.path().join("records/2026-10-02.jsonl"), lines(early, 3)).unwrap();
    let ids = |f: Filter| {
        records::read(home.path(), &f).iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>()
    };
    let (a, b, c) = (id(late - 60_000, 1), id(late, 2), id(early, 3));
    assert!(records::cursor_exists(home.path(), &b) && records::cursor_exists(home.path(), &c));
    assert_eq!(ids(Filter { limit: Some(5), before: Some(c.clone()), ..Filter::default() }), [b.clone(), a.clone()]);
    assert_eq!(ids(Filter { limit: Some(5), before: Some(b), ..Filter::default() }), [a]);
    assert!(ids(Filter { limit: Some(5), before: Some(id(late - 60_000, 1)), ..Filter::default() }).is_empty());

    // An id nobody has, and the id of a record that was pruned.
    assert!(!records::cursor_exists(home.path(), &id(START_MS + 5_000, 9)));
    assert!(!records::cursor_exists(home.path(), "rq_not-an-id"));
    records::prune(home.path(), parse_rfc3339("2026-10-02T00:00:00Z").unwrap(), std::time::Duration::from_secs(5))
        .unwrap();
    assert!(!records::cursor_exists(home.path(), &id(late, 2)), "pruned");
    assert!(records::cursor_exists(home.path(), &c));
}
