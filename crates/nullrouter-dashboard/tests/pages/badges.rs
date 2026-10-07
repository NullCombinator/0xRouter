//! Every status in contracts/style-guide.md "Status → badge" renders with its badge variant
//! (T057, FR-046): the table's words through `status`, each notice level through `notice`, and
//! every badge on every page and window of the fixture home whose words the table names.

use nullrouter_dashboard::components::{notice, status};
use nullrouter_dashboard::pages::Id;
use nullrouter_engine::testkit::homes;

use crate::common::{Dash, decode};

/// The contract's table, written out again here so a change to either side is caught.
const TABLE: [(&str, &[&str]); 5] = [
    ("success", &["active", "signed in", "polled", "served", "loaded"]),
    ("error", &["needs sign-in", "refused", "failed", "records not kept"]),
    ("warning", &["cooling", "stale", "pending first poll", "estimated", "fallback"]),
    ("default", &["pay-as-you-go", "disabled", "revoked", "default", "never", "not built yet"]),
    ("info", &["in progress"]),
];

/// The variant the table gives a badge's words, from their leading words.
fn variant(words: &str) -> Option<&'static str> {
    let w = words.trim().to_ascii_lowercase();
    TABLE.iter().find(|(_, starts)| starts.iter().any(|s| w.starts_with(s))).map(|(v, _)| *v)
}

/// Every badge on a page as `(variant, words)`.
fn badges(html: &str) -> Vec<(String, String)> {
    html.split("class=\"badge badge--")
        .skip(1)
        .map(|rest| {
            let (v, after) = rest.split_once('"').unwrap();
            let words = after.split("</span>").nth(1).unwrap_or_default();
            (v.to_owned(), decode(words).trim().to_owned())
        })
        .collect()
}

#[test]
fn each_status_word_renders_with_its_variant() {
    for (want, words) in TABLE {
        for w in words {
            let html = status(w).into_string();
            assert!(html.contains(&format!("class=\"badge badge--{want}\"")), "{w:?}: {html}");
        }
    }
    for (level, want) in [("error", "error"), ("warning", "warning"), ("note", "note")] {
        let html = notice(level, "x").into_string();
        assert!(html.contains(&format!("class=\"notice notice--{want}\"")), "{level} notice: {html}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_badge_on_a_page_follows_the_table() {
    let d = Dash::dashboard().await;
    let mut paths: Vec<String> = Id::ALL.iter().map(|id| id.path().to_owned()).collect();
    paths.extend(["/providers/anthropic".into(), "/providers/xai".into(), format!("/usage/records/{}", homes::fallback_id())]);
    let mut seen = 0;
    for path in &paths {
        for (v, words) in badges(&d.ok(path).await) {
            if let Some(want) = variant(&words) {
                assert_eq!(v, want, "{path}: {words:?}");
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "the fixture home shows statuses the table names");
}
