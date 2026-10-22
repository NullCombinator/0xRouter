//! US5 (spec 014, T041): three changes made from the browser, one refused, through the Console Log.
//!
//! The CLI's `dashboard log --json` prints `serde_json::to_value(&entries)` for
//! `change_log::read(home, 50)` (crates/nullrouter-cli/src/cmd/dashboard.rs, `fn log`). The CLI
//! binary is not run here: the test builds that same value in process from the same home and
//! compares the page and the raw log file against it. The page lists the changes newest first, the
//! JSON holds them in the same order, and no entry, page, response or log file holds the secret
//! pasted into the first add.

mod common;

use common::{Site, section, shows_instant, text_of};
use nullrouter_server::views::change_log;
use reqwest::StatusCode;
use serde_json::Value;

/// The secret pasted into the first add. It must reach the account store and nowhere else.
const MARKER: &str = "sk-marker-T041-secret";

/// The three changes, newest first: the order the page and the JSON both list them in.
/// `(op, subject, result)`.
const NEWEST_FIRST: [(&str, &str, &str); 3] = [
    ("accounts.add", "anthropic/spare", "refused"),
    ("keys.issue", "console-key", "done"),
    ("accounts.add", "anthropic/marked", "done"),
];

#[tokio::test(flavor = "multi_thread")]
async fn three_changes_one_refused_appear_in_order_on_the_page_and_in_the_cli_json() {
    let site = Site::start().await;

    // 1. An add with a marker secret: done.
    let marked = site
        .flow(
            "accounts/add",
            "/providers/anthropic",
            &[
                ("provider", "anthropic".to_owned()),
                ("name", "marked".to_owned()),
                ("secret", MARKER.to_owned()),
                ("order", String::new()),
            ],
        )
        .await;
    assert_eq!(marked.refused, None, "the marked account was refused");

    // 2. A client key: done.
    let issued = site
        .flow(
            "keys/issue",
            "/endpoint",
            &[
                ("name", "console-key".to_owned()),
                ("break", String::new()),
                ("harness", String::new()),
                ("adapter", String::new()),
            ],
        )
        .await;
    assert_eq!(issued.refused, None, "the key was refused");

    // 3. An add with an empty secret: refused.
    let spare = site
        .flow(
            "accounts/add",
            "/providers/anthropic",
            &[
                ("provider", "anthropic".to_owned()),
                ("name", "spare".to_owned()),
                ("secret", String::new()),
                ("order", String::new()),
            ],
        )
        .await;
    assert!(spare.refused.is_some(), "an empty secret was not refused");

    // `dashboard log --json`: the same value the CLI prints, from the same home.
    let entries = change_log::read(&site.d.home(), 50).expect("the change log reads");
    let cli: Value = serde_json::to_value(&entries).expect("the entries serialize");
    let cli_text = cli.to_string();
    let rows = cli.as_array().expect("the log is an array");
    assert_eq!(rows.len(), 3, "{cli}");
    for (row, (op, subject, result)) in rows.iter().zip(NEWEST_FIRST) {
        assert_eq!(row["op"].as_str(), Some(op), "{cli}");
        assert_eq!(row["subject"].as_str(), Some(subject), "{cli}");
        assert_eq!(row["result"].as_str(), Some(result), "{cli}");
    }

    // The Console Log page: its table body, one row per change, newest first.
    let (status, page) = site.get("/console-log").await;
    assert_eq!(status, StatusCode::OK, "/console-log");
    let body = section(&page, "<tbody>", "</tbody>");
    let page_rows: Vec<String> = body.split("<tr").skip(1).map(text_of).collect();
    assert_eq!(page_rows.len(), 3, "{page_rows:?}");
    for (i, (op, subject, result)) in NEWEST_FIRST.iter().enumerate() {
        let row = &page_rows[i];
        assert!(row.contains(op), "row {i} lacks its op {op}: {row}");
        assert!(row.contains(subject), "row {i} lacks its subject {subject}: {row}");
        assert!(row.contains(result), "row {i} lacks its result {result}: {row}");
        let text = rows[i]["text"].as_str().expect("the text is a string");
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(row.contains(&text), "row {i} lacks its text {text:?}: {row}");
        let at = rows[i]["at"].as_str().expect("the time is a string");
        assert!(shows_instant(&page, at), "row {i} does not show the time {at}");
    }

    // No entry, page or response holds the marker secret.
    assert!(!page.contains(MARKER), "the Console Log shows the secret");
    assert!(!cli_text.contains(MARKER), "the CLI's JSON holds the secret: {cli_text}");
    let bodies = site.seen.lock().unwrap().clone();
    for body in &bodies {
        assert!(!body.contains(MARKER), "a response carries the secret");
    }
    let raw = std::fs::read_to_string(change_log::path(&site.d.home())).expect("the log file exists");
    assert!(!raw.contains(MARKER), "dashboard-log.jsonl holds the secret");
    assert_eq!(raw.lines().count(), 3, "{raw}");
}
