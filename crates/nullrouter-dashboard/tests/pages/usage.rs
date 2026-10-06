//! What Usage says in the cases its story names (T036): the three slots with no digits, "Recent
//! Requests" with the newest 10, a missing record id as a 404 inside the frame with the CLI's
//! message, and an empty home.

use std::io::Write as _;

use nullrouter_dashboard::page::ViewName;
use nullrouter_dashboard::pages::usage;
use nullrouter_engine::testkit::homes;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{Dash, text_of};

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The `dashboard()` home and 60 more finished records on its oldest day (valid ULIDs sharing the
/// first record's time, in `i` order).
fn many() -> tempfile::TempDir {
    let dir = homes::dashboard();
    let base = homes::record_id(1);
    let prefix = &base[..base.len() - 3];
    let mut text = String::new();
    for i in 0..60usize {
        let id = format!("{prefix}Z{}{}", char::from(ALPHABET[i / 32]), char::from(ALPHABET[i % 32]));
        let at = format!("2026-10-01T10:00:{i:02}Z");
        for line in [
            json!({"v":1,"t":"open","id":id,"arrived":at,"agent":"ak_fixture1","style":"anthropic-messages",
                "op":"generate","type":"text","target":"sonnet"}),
            json!({"v":1,"t":"close","id":id,"outcome":"succeeded",
                "served_by":{"provider":"anthropic","account":"main","model":"claude-sonnet-4-5"},
                "ttft_ms":100.0,"total_ms":500.0,"usage":null,"break_handling":{"kind":"none"},"job":null}),
        ] {
            text += &line.to_string();
            text += "\n";
        }
    }
    let mut f = std::fs::OpenOptions::new().append(true).open(dir.path().join("records/2026-10-01.jsonl")).unwrap();
    f.write_all(text.as_bytes()).unwrap();
    dir
}

/// The `<div class="slot">` blocks of the page.
fn slots(html: &str) -> Vec<String> {
    html.split("<div class=\"slot\">").skip(1).map(|s| s.split("</div>").next().unwrap_or_default().to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_period_filter_the_stat_cards_and_the_graph_are_slots_with_no_numbers() {
    // Records exist, so a digit that leaks in from them would show.
    let d = Dash::dashboard().await;
    let html = d.ok("/usage").await;
    let found = slots(&html);
    assert_eq!(found.len(), 3, "the period filter, the stat cards and the topology graph");
    for slot in &found {
        assert!(!slot.chars().any(|c| c.is_ascii_digit()), "a slot shows no digits: {slot}");
        assert!(text_of(slot).contains("Arrives with the next dashboard slice."), "{slot}");
    }
    let text = text_of(&html);
    for title in ["Period filter", "Requests, input, cached, output, Est. Cost", "Topology graph"] {
        assert!(text.contains(title), "{title}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn recent_requests_lists_the_newest_ten_of_the_tables_read() {
    let d = Dash::start(many()).await;
    let list = d.view(ViewName::Records, json!({"limit": 50, "before": null})).await;
    let html = d.ok("/usage").await;
    let at = html.find("class=\"usage-recent\"").expect("Recent Requests is on the page");
    let section = &html[at..];
    let section = &section[..section.find("</section>").unwrap()];
    assert!(text_of(section).contains("Recent Requests"));
    let ids: Vec<&str> = section
        .split("class=\"usage-recent__row\"")
        .skip(1)
        .map(|r| {
            let from = r.find("href=\"/usage/records/").unwrap() + "href=\"/usage/records/".len();
            &r[from..from + r[from..].find('"').unwrap()]
        })
        .collect();
    let want: Vec<&str> = list.as_array().unwrap().iter().take(10).map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, want, "the newest 10, in the table's order");
    assert!(text_of(section).contains("in 1 204 out 388") || text_of(section).contains("not reported"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_record_is_a_404_inside_the_frame_with_the_clis_message() {
    let d = Dash::dashboard().await;
    let (status, html) = d.page("/usage/records/rq_nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(html.contains("class=\"sidebar\""), "the frame is drawn");
    assert!(html.contains("usage-row"), "the page stays under the message");
    let text = text_of(&html);
    assert!(text.contains("no record rq_nope"), "the CLI's message: {text}");
    assert!(text.contains("Not found"), "the window that says so: {text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_known_record_opens_its_window_over_the_page() {
    let d = Dash::dashboard().await;
    let id = homes::fallback_id();
    let html = d.ok(&format!("/usage/records/{id}?notices")).await;
    assert!(html.contains("class=\"modal\"") && html.contains("usage-row"));
    assert!(text_of(&html).contains(&id));
    assert!(html.contains("href=\"/usage?notices\""), "the window closes to the same page and query");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_home_says_there_are_no_records_yet() {
    let d = Dash::empty().await;
    let html = d.ok("/usage").await;
    let text = text_of(&html);
    assert!(text.contains(usage::EMPTY), "{text}");
    assert!(!html.contains("usage-row"), "no rows");
    assert!(!text.contains("Older"), "no Older link");
    assert_eq!(slots(&html).len(), 3, "the slots stay");
}
