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
async fn the_slots_hold_a_title_and_no_digits() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint").await;
    let slots = slots(&html);
    let cards = html.matches("<article class=\"key-card").count();
    assert!(cards > 0, "the fixture has keys");
    assert_eq!(slots.len(), 1 + cards, "Agent traffic, and Requests today on each card");
    assert!(slots[0].contains("Agent traffic"), "{slots:?}");
    for s in &slots {
        assert!(!s.chars().any(|c| c.is_ascii_digit()), "a slot shows no number: {s:?}");
        assert!(s.contains("Arrives with the next dashboard slice."), "{s:?}");
    }
    assert!(slots[1..].iter().all(|s| s.contains("Requests today")), "{slots:?}");
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
