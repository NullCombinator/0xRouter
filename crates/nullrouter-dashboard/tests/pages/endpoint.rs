//! What Endpoint & Key and Settings say where the CLI has no twin (T047): the slots hold no
//! numbers, Add Agent is disabled with nothing to submit, the adapters panel has no rows, and
//! Settings changes nothing.

use nullrouter_dashboard::pages::endpoint;

use crate::common::{Dash, text_of};

/// The text of each slot on the page.
fn slots(html: &str) -> Vec<String> {
    html.split("<div class=\"slot\"").skip(1).map(|s| text_of(s.split("</div>").next().unwrap_or_default())).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn no_slot_is_left_each_card_shows_its_requests_today() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint").await;
    let cards = html.matches("<article class=\"key-card").count();
    assert!(cards > 0, "the fixture has keys");
    assert!(slots(&html).is_empty(), "every spec 009 slot on Endpoint & Key is filled");
    assert_eq!(text_of(&html).matches("requests today").count(), cards, "one on each card");
    assert!(html.contains("landscape__svg"), "the landscape is drawn");
}

#[tokio::test(flavor = "multi_thread")]
async fn add_agent_is_disabled_and_names_the_cli_command() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint").await;
    let at = html.find("Add Agent").expect("the control is drawn");
    let button = &html[html[..at].rfind("<button").unwrap()..at];
    assert!(button.contains("disabled") && button.contains("type=\"button\""), "{button}");
    assert!(!html.contains("<form"), "nothing on the page submits");
    assert!(!html.contains("type=\"submit\"") && !html.contains("<script"));
    assert!(text_of(&html).contains("In the CLI: nullrouter keys issue <name>"));
    assert!(!html.contains("content_copy") && !html.contains("clipboard"), "no copy button");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_adapters_panel_says_they_are_not_built_and_lists_none() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint").await;
    let from = html.find("<details class=\"side-panel\"").expect("the panel is drawn");
    let panel = &html[from..from + html[from..].find("</details>").unwrap()];
    let text = text_of(panel);
    assert!(text.contains("Client adapters") && text.contains(endpoint::ADAPTERS), "{text}");
    assert!(!panel.contains("plugin-row") && !panel.contains("<li") && !panel.contains("<table"), "no rows: {panel}");
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_has_its_three_cards_and_nothing_to_change() {
    let d = Dash::dashboard().await;
    let html = d.ok("/settings").await;
    let text = text_of(&html);
    for card in ["Routing", "Local mode", "Dashboard", "Token issued"] {
        assert!(text.contains(card), "{card}\n{text}");
    }
    assert!(text.contains("Change it with nullrouter dashboard token"), "{text}");
    assert!(html.contains("<code class=\"code\">nullrouter dashboard token</code>"));
    assert!(!html.contains("<form") && !html.contains("<script"), "read-only");
}
