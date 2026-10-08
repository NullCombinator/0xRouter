//! Usage against `records list` and `records show` (T035, SC-001): the Requests table is the
//! first page of `records list --limit 50`, "Older" is `--before <last id>`, a row's window is
//! `records show <id>`, and a record the CLI words as not reported, in progress or interrupted is
//! worded the same way, never as zero.

use std::io::Write as _;
use std::sync::atomic::Ordering;
use std::time::Duration;

use nullrouter_dashboard::page::ViewName;
use nullrouter_engine::journal::Target;
use nullrouter_engine::testkit::homes;
use serde_json::{Value, json};

use crate::common::{Dash, home_with_traffic, shows_instant, text_of};

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

// ---------------------------------------------------------------------------------------------
// The CLI's words (crates/nullrouter-cli/src/cmd/records.rs)

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn ms(v: &Value) -> String {
    let tenths = |f: f64| (f * 10.0).round() as u64;
    v.as_f64().map_or_else(|| "-".into(), |f| format!("{}.{} ms", grouped(tenths(f) / 10), tenths(f) % 10))
}

fn count(v: &Value) -> String {
    v.as_u64().map_or_else(|| "not reported".into(), grouped)
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("-").to_owned()
}

fn who(v: &Value) -> String {
    match v["account"].as_str() {
        Some(a) => format!("{}/{a}", s(&v["provider"])),
        None => s(&v["provider"]),
    }
}

fn result_words(r: &Value) -> String {
    s(&r["outcome"]).replace('_', " ")
}

/// The reason of the attempt that served the record, else of the last one a placement chose.
fn why(r: &Value) -> String {
    let attempts = r["attempts"].as_array().cloned().unwrap_or_default();
    let by = attempts
        .iter()
        .rev()
        .find(|a| a["outcome"]["state"] == "ok")
        .or_else(|| attempts.iter().rev().find(|a| a["placement"]["reason"].is_string()));
    by.map_or_else(|| "-".into(), |a| s(&a["placement"]["reason"]))
}

fn attempt_outcome(a: &Value) -> String {
    let o = &a["outcome"];
    match o["state"].as_str() {
        Some("ok") => "ok".into(),
        Some("failed") => {
            let status = o["status"].as_u64().map_or_else(String::new, |n| format!("{n} "));
            format!("{status}{}: {}", s(&o["class"]).replace('_', " "), s(&o["reason"]))
        }
        Some("skipped") => format!("skipped: {}", s(&o["reason"])),
        Some("cancelled") => "cancelled".into(),
        _ => "in progress".into(),
    }
}

fn attempt_usage(u: &Value) -> String {
    [("in", "input"), ("out", "output"), ("cache r", "cache_read"), ("cache w", "cache_write")]
        .iter()
        .filter_map(|(label, key)| u[*key].as_u64().map(|n| format!("{label} {}", grouped(n).replace(' ', ","))))
        .collect::<Vec<_>>()
        .join(" · ")
}

// ---------------------------------------------------------------------------------------------
// Homes

/// The `dashboard()` home with 60 more finished records on its oldest day, so that 67 records
/// make two pages. Their ids share the first record's ULID time and differ in the last two
/// characters, which keeps them valid ULIDs that sort in `i` order.
fn many() -> tempfile::TempDir {
    let dir = homes::dashboard();
    let base = homes::record_id(1);
    let prefix = &base[..base.len() - 3];
    let mut text = String::new();
    for i in 0..60usize {
        let id = format!("{prefix}Z{}{}", char::from(ALPHABET[i / 32]), char::from(ALPHABET[i % 32]));
        let at = format!("2026-10-01T10:00:{i:02}Z");
        text += &json!({"v":1,"t":"open","id":id,"arrived":at,"agent":"ak_fixture1","style":"anthropic-messages",
            "op":"generate","type":"text","target":"sonnet"})
        .to_string();
        text += "\n";
        text += &json!({"v":1,"t":"close","id":id,"outcome":"succeeded",
            "served_by":{"provider":"anthropic","account":"main","model":"claude-sonnet-4-5"},
            "ttft_ms":100.0,"total_ms":500.0,"usage":null,"break_handling":{"kind":"none"},"job":null})
        .to_string();
        text += "\n";
    }
    append(&dir, &text);
    dir
}

/// Two records with a placement decision: one that stayed warm, one that moved.
fn with_decisions() -> (tempfile::TempDir, String, String) {
    let dir = homes::dashboard();
    let base = homes::record_id(1);
    let prefix = &base[..base.len() - 3];
    let (stayed, moved) = (format!("{prefix}Y00"), format!("{prefix}Y01"));
    let attempt = json!({"n":1,"provider":"anthropic","account":"max","model":"m","kind":"initial",
        "placement":{"reason":"cold_by_deficit","rank":0},"started":10.0,"ended":2220.0,"outcome":{"state":"ok"},
        "usage":{"input":18210,"output":512,"cache_read":null,"cache_write":18100},
        "dropped":[{"path":"metadata","reason":"no place in openai-chat"}],"forced":[["reasoning.effort","high"]]});
    let candidate = |provider: &str, account: &str, tier: &str, more: Value| {
        let mut c = json!({"provider": provider, "account": account, "model": "m", "tier": tier});
        for (k, v) in more.as_object().unwrap() {
            c[k] = v.clone();
        }
        c
    };
    let decisions = [
        (
            &stayed,
            "2026-10-01T12:00:00Z",
            json!({"kind":"warm","at":"2026-10-01T12:00:00Z","size_tokens":41200,
                "amortization_window":{"start":"2026-10-01T05:00:00.000Z","length":"5h"},
                "warm":{"provider":"anthropic","account":"main","model":"m","prefix_tokens":41200,"idle_s":38.0,"stayed":true},
                "order":[],"candidates":[]}),
        ),
        (
            &moved,
            "2026-10-01T12:30:00Z",
            json!({"kind":"cold","at":"2026-10-01T12:30:00Z","size_tokens":18400,
            "amortization_window":{"start":"2026-10-01T05:00:00.000Z","length":"5h"},
            "warm":{"provider":"anthropic","account":"max","model":"m","prefix_tokens":41200,"idle_s":38.0,
                "stayed":false,"moved_because":"reserve_floor"},
            "order":[1,0,2],
            "candidates":[
                candidate("anthropic","pro","subscription",json!({"pace":0.88,"share":0.39,"deficit_before":-91200})),
                candidate("anthropic","max","subscription",json!({"pace":1.42,"share":0.61,"deficit_before":91200})),
                candidate("openrouter","main","payg",json!({"price_now":3.0,"why_not":"reserve_floor"})),
            ]}),
        ),
    ];
    let mut text = String::new();
    for (id, at, decision) in decisions {
        for line in [
            json!({"v":1,"t":"open","id":id,"arrived":at,"agent":"ak_fixture1","style":"anthropic-messages",
                "op":"generate","type":"text","target":"sonnet"}),
            json!({"v":1,"t":"decision","id":id,"decision":decision}),
            json!({"v":1,"t":"attempt","id":id,"attempt":attempt}),
            json!({"v":1,"t":"close","id":id,"outcome":"succeeded",
                "served_by":{"provider":"anthropic","account":"max","model":"m"},"ttft_ms":640.0,"total_ms":2210.0,
                "usage":null,"break_handling":{"kind":"none"},"job":null}),
        ] {
            text += &line.to_string();
            text += "\n";
        }
    }
    append(&dir, &text);
    (dir, stayed, moved)
}

fn append(dir: &tempfile::TempDir, text: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(dir.path().join("records/2026-10-01.jsonl")).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

// ---------------------------------------------------------------------------------------------
// The table

/// The `<tr class="usage-row">` blocks of the page, in order.
fn rows(html: &str) -> Vec<String> {
    html.split("<tr class=\"usage-row\">")
        .skip(1)
        .map(|r| r.split("</tr>").next().unwrap_or_default().to_owned())
        .collect()
}

fn row_id(row: &str) -> String {
    let at = row.find("href=\"/usage/records/").expect("a row links to its record") + "href=\"/usage/records/".len();
    let rest = &row[at..];
    rest[..rest.find(['"', '?']).unwrap()].to_owned()
}

/// Asserts `row` shows what `records list` holds for `r`.
fn assert_row(row: &str, r: &Value) {
    let id = s(&r["id"]);
    assert_eq!(row_id(row), id);
    assert!(shows_instant(row, r["arrived"].as_str().unwrap()), "{id}: the arrival is on the row");
    let placed_on = if r["served_by"].is_null() { "-".to_owned() } else { who(&r["served_by"]) };
    let want = format!(
        "{} {} → {placed_on} {} {} {} {}",
        s(&r["agent"]["key"]),
        s(&r["target"]),
        why(r),
        ms(&r["ttft_ms"]),
        ms(&r["total_ms"]),
        result_words(r)
    );
    let text = text_of(row);
    assert!(text.contains(&want), "{id}: {want:?} is not in {text:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn page_one_is_records_list_limit_50() {
    let d = Dash::dashboard().await;
    let list = d.view(ViewName::Records, json!({"limit": 50, "before": null})).await;
    let list = list.as_array().unwrap();
    assert!(list.len() >= 5, "the fixture has its records: {}", list.len());
    let html = d.ok("/usage").await;
    let shown = rows(&html);
    assert_eq!(shown.len(), list.len());
    for (row, r) in shown.iter().zip(list) {
        assert_row(row, r);
    }
    let text = text_of(&html);
    assert!(
        text.contains("Newest first, 50 at a time · same as nullrouter records list --limit 50"),
        "the table says what it is"
    );
    assert!(!text.contains("Older"), "{} records are one page", list.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn older_is_records_list_before_the_last_id() {
    let d = Dash::start(many()).await;
    let first = d.view(ViewName::Records, json!({"limit": 50, "before": null})).await;
    let first = first.as_array().unwrap().clone();
    assert_eq!(first.len(), 50);
    let html = d.ok("/usage").await;
    assert_eq!(
        rows(&html).iter().map(|r| row_id(r)).collect::<Vec<_>>(),
        first.iter().map(|r| s(&r["id"])).collect::<Vec<_>>()
    );

    let last = s(&first[49]["id"]);
    assert!(html.contains(&format!("href=\"/usage?before={last}\"")), "Older links to ?before=<last id>");
    assert!(text_of(&html).contains("Older"));

    let next = d.view(ViewName::Records, json!({"limit": 50, "before": last})).await;
    let next = next.as_array().unwrap();
    let all = d.view(ViewName::Records, json!({"limit": 1000, "before": null})).await;
    let all = all.as_array().unwrap().len();
    assert!(all > 60, "the fixture's records and sixty more: {all}");
    assert_eq!(next.len(), all - 50, "{all} records, 50 on the first page");
    let page = d.ok(&format!("/usage?before={last}")).await;
    let shown = rows(&page);
    assert_eq!(shown.len(), next.len());
    for (row, r) in shown.iter().zip(next) {
        assert_row(row, r);
    }
    assert!(!text_of(&page).contains("Older"), "the last page has no Older link");
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_keep_the_query_in_their_links() {
    let d = Dash::start(many()).await;
    let first = d.view(ViewName::Records, json!({"limit": 50, "before": null})).await;
    let last = s(&first[49]["id"]);
    let page = d.ok(&format!("/usage?before={last}&notices")).await;
    let id = row_id(&rows(&page)[0]);
    assert!(page.contains(&format!("href=\"/usage/records/{id}?before={last}&amp;notices\"")), "{id}");
}

// ---------------------------------------------------------------------------------------------
// The window

/// Asserts the window shows what `records show` shows for `rec`.
fn assert_window(html: &str, rec: &Value, extra: &Value) {
    let id = s(&rec["id"]);
    let text = text_of(html);
    let has = |want: String| assert!(text.contains(&want), "{id}: {want:?} is not in the window\n{text}");
    has(id.clone());
    assert!(shows_instant(html, rec["arrived"].as_str().unwrap()), "{id}: the arrival");
    has(format!("Result {}", result_words(rec)));
    if !rec["agent"].is_null() {
        let key = s(&rec["agent"]["key"]);
        let name = extra["key_names"][key.as_str()].as_str().unwrap_or(&key).to_owned();
        match rec["agent"]["session"].as_str() {
            Some(sess) => has(format!("Agent {name} / session {sess}")),
            None => has(format!("Agent {name}")),
        }
    }
    has(format!("Door {} {} {}", s(&rec["style"]), s(&rec["op"]), s(&rec["model_type"])));
    if let Some(t) = rec["target"].as_str() {
        has(format!("Target {t}{}", if rec["unified_model"].is_null() { "" } else { " (unified)" }));
    }
    if !rec["served_by"].is_null() {
        has(format!("Served by {} {}", who(&rec["served_by"]), s(&rec["served_by"]["model"])));
    }
    has(format!("TTFT {} Total {}", ms(&rec["ttft_ms"]), ms(&rec["total_ms"])));
    let u = &rec["usage"];
    if u.is_null() {
        has("Usage not reported".into());
    } else {
        let estimated = if u["estimated"] == true { " (estimated)" } else { "" };
        has(format!(
            "Usage input {} output {} cache-read {} cache-write {}{estimated}",
            count(&u["input"]),
            count(&u["output"]),
            count(&u["cache_read"]),
            count(&u["cache_write"])
        ));
    }
    for a in rec["attempts"].as_array().unwrap() {
        let placed = match (a["placement"]["reason"].as_str(), a["placement"]["rank"].as_u64()) {
            (Some(reason), Some(rank)) => format!("{reason} (rank {rank}) "),
            _ => String::new(),
        };
        has(format!("{} {} {} {placed}{}", a["n"], who(a), s(&a["model"]), attempt_outcome(a)));
        if let (Some(from), Some(to)) = (a["started"].as_f64(), a["ended"].as_f64()) {
            let used = attempt_usage(&a["usage"]);
            has(format!("{:.2} s{}{used}", (to - from) / 1000.0, if used.is_empty() { "" } else { " " }));
        }
        for dropped in a["dropped"].as_array().unwrap() {
            has(format!("dropped {}: {}", s(&dropped["path"]), s(&dropped["reason"])));
        }
        for forced in a["forced"].as_array().unwrap() {
            has(format!(
                "forced {}: {}",
                s(&forced[0]),
                forced[1].as_str().map_or_else(|| forced[1].to_string(), str::to_owned)
            ));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rows_window_is_records_show() {
    let d = Dash::dashboard().await;
    let mut ids = vec![homes::fallback_id()];
    ids.extend((1..=4).map(homes::record_id));
    for id in ids {
        let (rec, extra) = d.view_full(ViewName::Record, json!({"id": id})).await;
        let html = d.ok(&format!("/usage/records/{id}")).await;
        assert_window(&html, &rec, &extra);
        assert!(html.contains("class=\"modal\""), "{id}: a window");
        assert!(html.contains("href=\"/usage\""), "{id}: the close link is the page");
        assert!(!rows(&html).is_empty(), "{id}: the page stays under the window");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fallback_shows_both_attempts_and_what_the_second_changed() {
    let d = Dash::dashboard().await;
    let html = d.ok(&format!("/usage/records/{}", homes::fallback_id())).await;
    let text = text_of(&html);
    for want in [
        "1 anthropic/main claude-sonnet-4-5 warm (rank 0) 429 rate limited: rate limited",
        "2 xai/work grok-4 fallback (rank 1) ok",
        "dropped messages[0].x_opt: no place in openai-chat",
        "forced reasoning.effort: high",
    ] {
        assert!(text.contains(want), "{want:?} is not in the window\n{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_stay_warm_decision_and_its_candidates_are_shown_as_records_show_prints_them() {
    let (dir, stayed, moved) = with_decisions();
    let d = Dash::start(dir).await;

    let html = d.ok(&format!("/usage/records/{stayed}")).await;
    assert!(
        text_of(&html).contains("Decision warm on anthropic/main · prefix 41.2k · idle 38 s · stayed"),
        "{}",
        text_of(&html)
    );
    let (rec, extra) = d.view_full(ViewName::Record, json!({"id": stayed})).await;
    assert_window(&html, &rec, &extra);

    let html = d.ok(&format!("/usage/records/{moved}")).await;
    let text = text_of(&html);
    for want in [
        "Decision cold · size 18.4k · amortization window from",
        "5h long",
        "Warm anthropic/max · prefix 41.2k · idle 38 s · moved: reserve_floor on anthropic/max",
        "# account tier eligible pace share deficit price",
        "0 anthropic/max subscription yes 1.42 61% +91.2k",
        "1 anthropic/pro subscription yes 0.88 39% -91.2k",
        "2 openrouter/main payg no: reserve floor 3.00",
        "1 anthropic/max m cold_by_deficit (rank 0) ok",
        "2.21 s in 18,210 · out 512 · cache w 18,100",
        "dropped metadata: no place in openai-chat",
        "forced reasoning.effort: high",
    ] {
        assert!(text.contains(want), "{want:?} is not in the window\n{text}");
    }
    assert!(shows_instant(&html, "2026-10-01T05:00:00Z"), "the amortization window's start is a <time>");
    let (rec, extra) = d.view_full(ViewName::Record, json!({"id": moved})).await;
    assert_window(&html, &rec, &extra);
}

// ---------------------------------------------------------------------------------------------
// Not reported, in progress, interrupted

#[tokio::test(flavor = "multi_thread")]
async fn usage_not_reported_is_said_so_never_zero() {
    let d = Dash::dashboard().await;
    for n in [2, 4] {
        let id = homes::record_id(n);
        let (rec, _) = d.view_full(ViewName::Record, json!({"id": id})).await;
        assert!(rec["usage"].is_null(), "the fixture's record {n} reports no usage");
        let text = text_of(&d.ok(&format!("/usage/records/{id}")).await);
        assert!(text.contains("Usage not reported"), "{text}");
        assert!(!text.contains("input 0"), "{text}");
        let recent = text_of(&d.ok("/usage").await);
        assert!(recent.contains("not reported"), "the table's side list says so too: {recent}");
    }
}

/// The unfinished records of the fixture, in the words of the view, before and after a server is
/// running: a request with no `close` is cut short or in flight, and its times are `-`.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_with_no_close_is_worded_as_records_words_it() {
    let dir = homes::dashboard();
    homes::unfinished(dir.path());
    let d = Dash::start(dir).await;
    check_unfinished(&d).await;

    // Appended after the server started, the records are still open.
    let d = Dash::dashboard().await;
    homes::unfinished(d.dir.path());
    check_unfinished(&d).await;
}

async fn check_unfinished(d: &Dash) {
    let list = d.view(ViewName::Records, json!({"limit": 50, "before": null})).await;
    let html = d.ok("/usage").await;
    let shown = rows(&html);
    let list = list.as_array().unwrap();
    assert_eq!(shown.len(), list.len());
    for (row, r) in shown.iter().zip(list) {
        assert_row(row, r);
    }
    for n in [5, 6] {
        let id = homes::record_id(n);
        let (rec, extra) = d.view_full(ViewName::Record, json!({"id": id})).await;
        let words = result_words(&rec);
        assert!(["in progress", "interrupted"].contains(&words.as_str()), "record {n} is {words:?}");
        let html = d.ok(&format!("/usage/records/{id}")).await;
        assert_window(&html, &rec, &extra);
        let text = text_of(&html);
        if rec["ttft_ms"].is_null() && rec["total_ms"].is_null() {
            assert!(text.contains("TTFT - Total -"), "no times are shown as -, never as zero: {text}");
        }
        assert!(!text.contains(" 0.0 ms"), "{text}");
    }
}

// ---------------------------------------------------------------------------------------------
// Records not kept

#[tokio::test(flavor = "multi_thread")]
async fn records_not_kept_is_the_check_notice_above_the_page() {
    let d = Dash::dashboard().await;
    d.engine.journal.faults().fail_writes.store(true, Ordering::Relaxed);
    d.engine.journal.append(Target::Records { day: "2026-10-05".into() }, "open", json!({"id": "rq_unkept"}));
    let mut kept = true;
    for _ in 0..100 {
        kept = d.engine.journal.health().kept;
        if !kept {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!kept, "the journal could not write");

    let check = d.view(ViewName::Check, json!({})).await;
    let mine: Vec<String> = check["notices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["subject"] == "usage")
        .map(|n| n["text"].as_str().unwrap().split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    assert!(mine.iter().any(|t| t.contains("records not kept")), "{mine:?}");
    let text = text_of(&d.ok("/usage").await);
    for notice in mine {
        assert!(text.contains(&notice), "{notice:?} is above the Usage page\n{text}");
    }
    d.engine.journal.faults().fail_writes.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------------------------
// The period filter and the five cards against `usage` (spec 010, T014, SC-001)

const PERIODS: [&str; 6] = ["today", "24h", "7d", "30d", "60d", "all"];

fn comma(n: u64) -> String {
    grouped(n).replace(' ', ",")
}

/// The stat cards' text, which is where the totals appear (the Requests table and Recent Requests
/// show records, which carry their own numbers).
fn stat_cards(html: &str) -> String {
    let at = html.find("class=\"usage-stats\"").expect("the stat cards are on the page");
    let rest = &html[at..];
    text_of(&rest[..rest.find("class=\"usage-overview__pair\"").unwrap_or(rest.len())])
}

fn assert_cards_equal_usage(html: &str, u: &Value, what: &str) {
    let text = stat_cards(html);
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    for (label, value) in [
        ("Total Requests", comma(n(&u["requests"]))),
        ("Total Input Tokens", comma(n(&u["tokens"]["input"]))),
        ("Cached Tokens", comma(n(&u["tokens"]["cached"]))),
        ("Output Tokens", comma(n(&u["tokens"]["output"]))),
        ("Est. Cost", format!("~${:.2}", u["cost"]["usd"].as_f64().unwrap())),
    ] {
        assert!(text.contains(&format!("{label} {value}")), "{what}: {label} {value}\n{text}");
    }
    assert!(text.contains(u["cost"]["label"].as_str().unwrap()), "{what}: the label\n{text}");
    assert!(text.contains(u["cost"]["note"].as_str().unwrap()), "{what}: the note\n{text}");
    if n(&u["requests"]) == 0 {
        assert!(text.contains("No requests in this period"), "{what}\n{text}");
    } else if n(&u["in_flight"]) > 0 || n(&u["not_reported"]) > 0 {
        let small = format!("{} in flight · {} not reported", comma(n(&u["in_flight"])), comma(n(&u["not_reported"])));
        assert!(text.contains(&small), "{what}: {small}\n{text}");
    }
    let un = &u["cost"]["unpriced"];
    if n(&un["requests"]) > 0 {
        assert!(text.contains(&format!("{} not priced", comma(n(&un["requests"])))), "{what}: not priced\n{text}");
        for (k, label) in
            [("no_price", "no price"), ("account_gone", "account gone"), ("no_output_price", "no output price")]
        {
            if n(&un[k]) > 0 {
                assert!(text.contains(&format!("{} {label}", n(&un[k]))), "{what}: {label}\n{text}");
            }
        }
    } else {
        assert!(!text.contains("not priced"), "{what}: nothing is unpriced\n{text}");
    }
}

/// The value of the one filter button marked pressed.
fn pressed(html: &str) -> Vec<String> {
    html.split("<button ")
        .skip(1)
        .map(|b| b.split('>').next().unwrap_or_default())
        .filter(|b| b.contains("aria-pressed=\"true\""))
        .filter_map(|b| b.split("value=\"").nth(1).and_then(|v| v.split('"').next()).map(str::to_owned))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn each_period_shows_what_usage_prints() {
    let d = Dash::dashboard().await;
    for period in PERIODS {
        let html = d.ok(&format!("/usage?period={period}")).await;
        let u = d.view(ViewName::Usage, json!({"period": period})).await;
        assert_cards_equal_usage(&html, &u, period);
        assert_eq!(pressed(&html), [period], "{period}: the chosen one is marked");
        assert!(!text_of(&html).contains("Unknown period"), "{period}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_period_is_today_and_an_unknown_one_says_so() {
    let d = Dash::dashboard().await;
    let today = d.view(ViewName::Usage, json!({"period": "today"})).await;
    let plain = d.ok("/usage").await;
    assert_cards_equal_usage(&plain, &today, "no period");
    assert_eq!(pressed(&plain), ["today"]);
    assert!(!text_of(&plain).contains("Unknown period"));

    let bogus = d.ok("/usage?period=bogus").await;
    assert_cards_equal_usage(&bogus, &today, "period=bogus");
    assert_eq!(pressed(&bogus), ["today"]);
    assert!(text_of(&bogus).contains("Unknown period; showing Today"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_period_shows_zeros_and_says_so() {
    let d = Dash::empty().await;
    let html = d.ok("/usage?period=7d").await;
    let u = d.view(ViewName::Usage, json!({"period": "7d"})).await;
    assert_eq!(u["requests"], 0);
    assert_cards_equal_usage(&html, &u, "empty");
    let text = stat_cards(&html);
    assert!(text.contains("Total Requests 0"), "{text}");
    assert!(text.contains("No requests in this period"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_filter_is_a_get_form_with_the_six_periods_and_no_script() {
    let d = Dash::dashboard().await;
    let html = d.ok("/usage").await;
    assert!(html.contains("method=\"get\""), "a GET form");
    for (value, label) in
        [("today", "Today"), ("24h", "24h"), ("7d", "7D"), ("30d", "30D"), ("60d", "60D"), ("all", "All")]
    {
        assert!(html.contains(&format!("name=\"period\" value=\"{value}\"")), "{value}");
        assert!(text_of(&html).contains(label), "{label}");
    }
    assert!(!html.contains("<script"), "no script");
}

/// The topology graph's `<g>` nodes: (provider, count, edge class) in page order.
fn topology_nodes(html: &str) -> Vec<(String, String)> {
    let svg = html.split("<svg class=\"topology\"").nth(1).map_or("", |s| s.split("</svg>").next().unwrap_or_default());
    svg.split("<title>")
        .skip(1)
        .map(|g| {
            let id = g.split("</title>").next().unwrap_or_default().to_owned();
            let count = g
                .split("topology__count")
                .nth(1)
                .and_then(|c| c.split('>').nth(1))
                .and_then(|c| c.split('<').next())
                .unwrap_or_default();
            (id, count.to_owned())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_topology_graph_has_the_providers_and_counts_usage_prints_with_the_edges_latency_implies() {
    let d = Dash::start(home_with_traffic()).await;
    let latency = d.view(ViewName::Latency, json!({})).await;
    for period in PERIODS {
        let html = d.ok(&format!("/usage?period={period}")).await;
        let u = d.view(ViewName::Usage, json!({"period": period})).await;
        let want: Vec<(String, String)> = u["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["id"].as_str().unwrap().to_owned(), p["requests"].to_string()))
            .collect();
        assert_eq!(topology_nodes(&html), want, "{period}");
        let failed = u["providers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| {
                latency["providers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["id"] == p["id"] && r["last"]["result"] == "failed")
            })
            .count();
        assert_eq!(
            html.matches("topology__edge--failed").count(),
            failed,
            "{period}: red edges are the failed last responses"
        );
        let text = text_of(&html);
        assert!(!want.is_empty() == text.contains("last response · last 24 h"), "{period}: the label");
        assert!(!text.to_lowercase().contains("in flight now"), "{period}: no in-flight count on the graph");
    }
}
