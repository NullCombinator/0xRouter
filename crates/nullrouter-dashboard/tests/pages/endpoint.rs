//! What Endpoint & Key and Settings say where the CLI has no twin (T047): the slots hold no
//! numbers, Add Agent opens the issue window, each key card carries its tag, break and adapter
//! forms and a Revoke link, and Settings changes nothing.

use nullrouter_dashboard::page::ViewName;
use serde_json::json;

use crate::common::{Dash, text_of};

/// The text of each slot on the page.
fn slots(html: &str) -> Vec<String> {
    html.split("<div class=\"slot\"").skip(1).map(|s| text_of(s.split("</div>").next().unwrap_or_default())).collect()
}

/// The key cards of the page, each as the HTML between its `<article>` and `</article>`.
fn key_cards(html: &str) -> Vec<&str> {
    html.split("<article class=\"key-card").skip(1).map(|s| s.split("</article>").next().unwrap_or_default()).collect()
}

/// The first `<form>` posting to `action`, through its `</form>`.
fn form_to<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html.find(&format!("action=\"{action}\"")).unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").expect("the action belongs to a form");
    let end = at + html[at..].find("</form>").expect("the form closes");
    &html[start..end]
}

/// The form token a form carries: 32 hex digits.
fn token_of(form: &str) -> &str {
    let key = "name=\"_t\" value=\"";
    let at = form.find(key).expect("the form carries _t") + key.len();
    let token = &form[at..at + 32];
    assert!(token.bytes().all(|c| c.is_ascii_hexdigit()), "{token}");
    token
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
async fn add_agent_opens_a_window_whose_form_issues_a_key() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint").await;
    let at = html.find("Add Agent").expect("the control is drawn");
    let link = &html[html[..at].rfind("<a ").unwrap()..at];
    assert!(link.contains("href=\"/endpoint?window=add\"") && !link.contains("disabled"), "{link}");

    let window = d.ok("/endpoint?window=add").await;
    let form = form_to(&window, "/do/keys/issue");
    assert_eq!(token_of(form).len(), 32);
    assert!(form.contains("name=\"_back\" value=\"/endpoint\""), "{form}");
    assert!(form.contains("name=\"_window\" value=\"add\""), "{form}");
    for field in ["name", "harness", "break", "adapter"] {
        assert!(form.contains(&format!("name=\"{field}\"")), "{field}: {form}");
    }
    assert!(!window.contains("<script"), "no script");
}

#[tokio::test(flavor = "multi_thread")]
async fn active_key_cards_have_tag_break_and_adapter_forms_and_a_revoke_link() {
    let d = Dash::dashboard().await;
    let keys = d.view(ViewName::Keys, json!({})).await;
    let keys = keys.as_array().unwrap();
    let html = d.ok("/endpoint").await;
    let cards = key_cards(&html);
    assert_eq!(cards.len(), keys.len(), "one card per key");
    let mut active = 0;
    for (card, k) in cards.iter().zip(keys) {
        let id = k["id"].as_str().unwrap();
        let what = format!("key {id}");
        if k["revoked"].is_string() {
            // A revoked key can't be changed: no forms, no Revoke link.
            for action in ["/do/keys/tag", "/do/keys/set-break", "/do/keys/set-adapter"] {
                assert!(!card.contains(action), "{what} is revoked, so no {action}\n{card}");
            }
            assert!(!card.contains("window=revoke") && !card.contains(">Revoke<"), "{what}: no Revoke\n{card}");
            continue;
        }
        active += 1;
        for (action, field) in
            [("/do/keys/tag", "text"), ("/do/keys/set-break", "behaviour"), ("/do/keys/set-adapter", "harness")]
        {
            let form = form_to(card, action);
            assert!(form.contains(&format!("name=\"key\" value=\"{id}\"")), "{what}: {action}\n{form}");
            assert!(form.contains(&format!("name=\"{field}\"")), "{what}: {action} has {field}\n{form}");
        }
        assert!(card.contains("window=revoke") && card.contains(&format!("key={id}")), "{what}: revoke\n{card}");
        assert!(card.contains(">Revoke<"), "{what}: the Revoke link\n{card}");
    }
    assert!(active > 0, "the fixture has an active key");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_opens_the_confirm_window_with_what_is_lost() {
    let d = Dash::dashboard().await;
    let keys = d.view(ViewName::Keys, json!({})).await;
    let id = keys[0]["id"].as_str().unwrap();
    let html = d.ok(&format!("/endpoint?window=revoke&key={id}")).await;
    let loss = nullrouter_server::ops::describe_loss("keys.revoke", id).unwrap();
    assert!(text_of(&html).contains(&loss), "{loss}\n{}", text_of(&html));
    let form = form_to(&html, "/do/keys/revoke");
    assert!(form.contains(&format!("name=\"key\" value=\"{id}\"")), "{form}");
    assert!(form.contains("name=\"_window\" value=\"revoke\""), "{form}");
    assert!(
        form.contains(&format!("name=\"_back\" value=\"/endpoint?key={id}\"")),
        "a refusal reopens this window: {form}"
    );
    assert!(html.contains("class=\"modal\"") && !html.contains("<script"), "a window, no script");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_without_a_key_to_revoke_opens_nothing() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint?window=revoke").await;
    assert!(!html.contains("/do/keys/revoke") && !html.contains("class=\"modal\""), "{html}");
}

/// The adapters panel lists hermes as built-in and has the install, review-settings and alert forms,
/// all posting to their ops with the token and returning here; no script runs.
#[tokio::test(flavor = "multi_thread")]
async fn the_adapters_panel_lists_the_harnesses_and_has_the_install_and_review_forms() {
    let d = Dash::dashboard().await;
    let html = d.ok("/endpoint").await;
    let from = html.find("<details class=\"side-panel\"").expect("the panel is drawn");
    let panel = &html[from..from + html[from..].find("</details>").unwrap()];
    let text = text_of(panel);
    assert!(text.contains("Client adapters") && text.contains("hermes") && text.contains("built-in"), "{text}");
    for (action, field) in [
        ("/do/adapters/install", "name=\"path\""),
        ("/do/catalogue/install", "name=\"harness\""),
        ("/do/adapters/review-settings", "name=\"model\""),
    ] {
        let form = form_to(panel, action);
        assert!(form.contains(field) && form.contains("name=\"_back\" value=\"/endpoint\""), "{action}: {form}");
        assert!(form.contains("name=\"_t\" value=\""), "{action} carries the token: {form}");
    }
    assert!(!html.contains("<script"), "no script");
    // Look's panel: the third-party section names the CLI listing, the gear opens the settings window.
    assert!(text.contains("Client adapters") && text.contains("In the CLI: nullrouter adapters list"), "{text}");
    assert!(text.contains("Built in · 1") && text.contains("hermes") && text.contains("built-in"), "{text}");
    assert!(panel.contains(&format!("for=\"{}\"", nullrouter_dashboard::pages::endpoint::SETTINGS)), "the gear opens the settings window");
    let settings = text_of(&html[html.find(&format!("id=\"{}\"", nullrouter_dashboard::pages::endpoint::SETTINGS)).expect("the settings window")..]);
    assert!(settings.contains("Client adapter settings") && settings.contains("nullrouter adapters install"), "{settings}");
}

/// A clear or remove window opens only for a named version, and names what it loses.
#[tokio::test(flavor = "multi_thread")]
async fn the_adapter_clear_and_remove_windows_need_a_version_and_post_it() {
    let d = Dash::dashboard().await;
    for (window, action, loss) in [
        ("clear-adapter", "/do/adapters/clear", "adapters.clear"),
        ("remove-adapter", "/do/adapters/remove", "adapters.remove"),
    ] {
        let none = d.ok(&format!("/endpoint?window={window}&harness=claude-code")).await;
        assert!(!none.contains(action), "{window}: no version, no window");
        let html = d.ok(&format!("/endpoint?window={window}&harness=claude-code&version=v1")).await;
        let form = form_to(&html, action);
        assert!(form.contains(&format!("name=\"_window\" value=\"{window}\"")), "{form}");
        assert!(
            form.contains("name=\"harness\" value=\"claude-code\"") && form.contains("name=\"version\" value=\"v1\"")
        );
        assert!(html.contains("class=\"modal\""), "a window");
        let subject = "claude-code v1";
        let want = nullrouter_server::ops::describe_loss(loss, subject).unwrap();
        assert!(text_of(&html).contains(&want), "{want}");
    }
}

/// Settings shows its cards, and its two changes are forms that post to their ops and return here;
/// the token is still changed in the CLI only.
#[tokio::test(flavor = "multi_thread")]
async fn settings_has_its_cards_and_the_break_and_window_forms() {
    let d = Dash::dashboard().await;
    let html = d.ok("/settings").await;
    let text = text_of(&html);
    for card in ["Routing", "Change", "Local mode", "Dashboard", "Token issued"] {
        assert!(text.contains(card), "{card}\n{text}");
    }
    assert!(text.contains("Change it with nullrouter dashboard token"), "{text}");
    assert!(html.contains("<code class=\"code\">nullrouter dashboard token</code>"));

    let brk = form_to(&html, "/do/behaviour/set-break");
    assert!(brk.contains("name=\"behaviour\"") && brk.contains("name=\"_back\" value=\"/settings\""), "{brk}");
    assert!(brk.contains("<option value=\"restart\"") && brk.contains("<option value=\"error_event\""), "{brk}");
    assert!(brk.contains("name=\"_t\" value=\""), "the form carries the token: {brk}");

    let window = form_to(&html, "/do/routing/window");
    assert!(window.contains("name=\"duration\"") && window.contains("name=\"_back\" value=\"/settings\""), "{window}");
    assert!(window.contains("name=\"_t\" value=\""), "the form carries the token: {window}");
    assert!(!html.contains("<script"), "no script");
}

/// Settings shows the private-endpoints state; turning it on goes through the confirm window with
/// the CLI's question, and the form that confirms posts `on`.
#[tokio::test(flavor = "multi_thread")]
async fn private_endpoints_turn_on_through_a_confirm_window() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    assert_eq!(check["allow_private_endpoints"], false, "the fixture leaves it off");
    let html = d.ok("/settings").await;
    assert!(html.contains("href=\"/settings?window=private-endpoints\""), "{html}");
    assert!(!html.contains("/do/plugins/private-endpoints"), "off: turning on needs the window");

    let window = d.ok("/settings?window=private-endpoints").await;
    let loss = nullrouter_server::ops::describe_loss("plugins.private_endpoints", "on").unwrap();
    assert!(text_of(&window).contains(&loss), "{loss}");
    let form = form_to(&window, "/do/plugins/private-endpoints");
    assert!(form.contains("name=\"on\" value=\"on\"") && form.contains("name=\"_window\" value=\"private-endpoints\""));
    assert!(form.contains("name=\"_back\" value=\"/settings\""), "{form}");
}
