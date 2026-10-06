//! Endpoint & Key against `check --json` and `keys list`, and Settings against `behaviour show`,
//! `check` and `dashboard status` (T047, SC-001): every scalar of a fact the page shows is on the
//! page, instants through `<time datetime>`. Neither page carries a full key or the token.

use nullrouter_dashboard::page::ViewName;
use nullrouter_engine::files::DashboardToken;
use serde_json::{Value, json};

use crate::common::{Dash, TOKEN, assert_fields, assert_shows, text_of};

/// The key cards of the page, each as its own HTML.
fn key_cards(html: &str) -> Vec<String> {
    let card = |s: &str| s.split("</article>").next().unwrap_or_default().to_owned();
    html.split("<article class=\"key-card").skip(1).map(card).collect()
}

/// The digests `keys.toml` holds: what must never reach a page.
fn digests(dash: &Dash) -> Vec<String> {
    let text = std::fs::read_to_string(dash.dir.path().join("keys.toml")).unwrap();
    let quoted = |l: &str| l.strip_prefix("digest = \"")?.strip_suffix('"').map(str::to_owned);
    text.lines().filter_map(quoted).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_is_what_check_says_and_a_served_page_never_calls_it_configured() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    let html = d.ok("/endpoint").await;
    let text = text_of(&html);
    assert_eq!(check["endpoint_source"], "server", "the fixture serves");
    assert_shows(&html, &check["endpoint"], "endpoint");
    assert!(!text.contains("configured; no server running"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_key_card_shows_what_keys_list_shows() {
    let d = Dash::dashboard().await;
    let keys = d.view(ViewName::Keys, json!({})).await;
    let keys = keys.as_array().unwrap();
    let html = d.ok("/endpoint").await;
    let cards = key_cards(&html);
    assert!(!keys.is_empty(), "the fixture has keys");
    assert_eq!(cards.len(), keys.len(), "one card per key");
    for (card, k) in cards.iter().zip(keys) {
        let what = format!("key {}", k["id"]);
        assert_fields(card, k, &["id", "name", "key", "created"], &what);
        let text = text_of(card);
        match k["revoked"].as_str() {
            Some(_) => assert_shows(card, &k["revoked"], &what),
            None => assert!(!text.contains("revoked"), "{what} is not revoked: {text}"),
        }
        let behaviour = k["break"].as_str().unwrap_or("default");
        assert!(text.contains(&format!("break {behaviour}")), "{what}: break {behaviour}\n{text}");
        match k["last_used"].as_str() {
            Some(_) => assert_shows(card, &k["last_used"], &what),
            None => assert!(text.contains("last used never"), "{what} was never used: {text}"),
        }
        assert!(text.contains("Requests today"), "{what} has its slot");
    }
    assert!(keys.iter().any(|k| k["last_used"].is_string()), "a fixture key has records");
    assert!(keys.iter().any(|k| k["last_used"].is_null()), "a fixture key has none");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_home_says_there_are_no_agent_keys_and_how_to_add_one() {
    let d = Dash::empty().await;
    let html = d.ok("/endpoint").await;
    let text = text_of(&html);
    assert!(text.contains("No agent keys."), "{text}");
    assert!(text.contains("Run nullrouter keys issue <name>"), "{text}");
    assert!(html.contains("<code class=\"code\">nullrouter keys issue &lt;name&gt;</code>"), "the command is code");
    assert!(key_cards(&html).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn neither_page_carries_a_full_key_or_the_token() {
    let d = Dash::dashboard().await;
    let digests = digests(&d);
    assert!(!digests.is_empty());
    for path in ["/endpoint", "/settings"] {
        let html = d.ok(path).await;
        assert!(!html.contains(TOKEN), "{path} shows the token");
        let token_digest = DashboardToken::digest_of(TOKEN);
        assert!(!html.contains(&token_digest), "{path} shows the token's digest");
        assert!(!html.contains("0r-"), "{path} shows a key");
        for digest in &digests {
            assert!(!html.contains(digest.as_str()), "{path} shows a key's digest");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_shows_behaviour_the_home_and_the_dashboard_status() {
    let d = Dash::dashboard().await;
    let html = d.ok("/settings").await;
    let text = text_of(&html);

    let behaviour = d.view(ViewName::Behaviour, json!({})).await;
    let settings = behaviour.as_object().unwrap();
    assert!(!settings.is_empty());
    for (name, v) in settings {
        let tag = if v["default"] == true { " (default)" } else { "" };
        let want = format!("{name} {}{tag}", v["value"].as_str().unwrap());
        assert!(text.contains(&want), "{want:?}\n{text}");
    }

    let check = d.view(ViewName::Check, json!({})).await;
    assert_shows(&html, &check["home"], "home");

    let status = d.view(ViewName::Dashboard, json!({})).await;
    let listen = status["listen"].as_str().unwrap();
    let want = match (status["server"].as_str().unwrap(), status["enabled"] == true, status["serving"] == true) {
        ("running", false, _) => "off (config.toml [dashboard] enabled = false)".to_owned(),
        ("running", true, true) => format!("on, listening on {listen}"),
        ("running", true, false) => format!("on, not listening: {}", status["error"].as_str().unwrap_or(listen)),
        (_, true, _) => format!("no server running; config.toml: on, {listen}"),
        (_, false, _) => "no server running; config.toml: off".to_owned(),
    };
    assert!(text.contains(&format!("Status {want}")), "{want:?}\n{text}");
    assert_shows(&html, &Value::String(listen.to_owned()), "listen");
    assert_eq!(status["token_issued"], "2026-10-06T08:00:00Z");
    assert_shows(&html, &status["token_issued"], "token issued");
    assert!(text.contains("Change it with nullrouter dashboard token"), "{text}");
}
