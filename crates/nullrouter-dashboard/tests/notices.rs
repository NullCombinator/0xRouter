//! The housekeeping panel and the notices above each page (T041, User Story 6): `?notices` on any
//! page lists every `check --json` notice and every account that needs the operator, in `accounts
//! list`'s words; each notice also stands above the page its `subject` names; an empty home's
//! panel says "No notices."; the chat box is disabled and says it is not built.

mod common;

use common::{Dash, text_of};
use nullrouter_dashboard::frame::CHAT;
use nullrouter_dashboard::page::ViewName;
use nullrouter_dashboard::pages::Id;
use serde_json::{Value, json};

/// The HTML of the element that opens at `class="<class>"`, up to its closing tag `</tag>`.
fn element<'a>(html: &'a str, class: &str, tag: &str) -> Option<&'a str> {
    let start = html.find(&format!("class=\"{class}\""))?;
    let end = html[start..].find(&format!("</{tag}>")).map_or(html.len(), |e| start + e);
    Some(&html[start..end])
}

/// The text of the notices above the page: the run of notice elements inside `.notices`.
fn above_page(html: &str) -> String {
    let Some(start) = html.find("<div class=\"notices\">") else { return String::new() };
    let mut rest = &html[start + "<div class=\"notices\">".len()..];
    let mut out = String::new();
    while rest.starts_with("<div class=\"notice") {
        let end = rest.find("</div>").expect("a notice closes") + "</div>".len();
        out.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    text_of(&out)
}

/// The panel: from its section to the end of the chat box.
fn panel(html: &str) -> String {
    let start = html.find("class=\"house-panel\"").expect("the panel is open");
    let rest = &html[start..];
    let end = rest.find("</section>").expect("the panel closes");
    rest[..end].to_owned()
}

fn needs_action(a: &Value) -> bool {
    matches!(a["state"].as_str(), Some("needs_sign_in" | "refused" | "refreshing"))
        || a["state_text"].as_str().is_some_and(|t| t.starts_with("cooling"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_panel_lists_every_notice_and_every_account_needing_action_on_any_page() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    let accounts = d.view(ViewName::Accounts, json!({"provider": null})).await;
    let notices = check["notices"].as_array().expect("check lists notices");
    assert!(!notices.is_empty(), "the fixture home has notices");
    let needing: Vec<&Value> = accounts.as_array().unwrap().iter().filter(|a| needs_action(a)).collect();
    assert!(!needing.is_empty(), "the fixture home has an account needing sign-in");

    for id in Id::ALL {
        let html = d.ok(&format!("{}?notices", id.path())).await;
        let text = text_of(&panel(&html));
        for n in notices {
            let want = n["text"].as_str().unwrap();
            assert!(text.contains(&text_of(want)), "{}: {want:?} is not in the panel\n{text}", id.path());
        }
        for a in &needing {
            let who = format!("{}/{}", a["provider"].as_str().unwrap(), a["name"].as_str().unwrap());
            assert!(text.contains(&who), "{}: {who} is not in the panel", id.path());
            assert!(text.contains(a["state_text"].as_str().unwrap()), "{}: {who}'s state words", id.path());
        }
        assert!(text.contains(CHAT), "{}: the chat box says it is not built", id.path());
        let chat = element(&html, "house-panel__chat", "div").unwrap();
        assert!(chat.contains(" disabled"), "{}: the chat box is disabled", id.path());
        assert!(!chat.contains("<form"), "{}: the chat box sends nothing", id.path());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_notice_stands_above_the_page_its_subject_names() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    for id in Id::ALL {
        let html = d.ok(id.path()).await;
        let above = above_page(&html);
        for n in check["notices"].as_array().unwrap() {
            let want = text_of(n["text"].as_str().unwrap());
            let here = id.subject().is_some_and(|s| n["subject"] == s);
            assert_eq!(above.contains(&want), here, "{}: {want:?} (subject {})", id.path(), n["subject"]);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_home_has_no_notices() {
    let d = Dash::empty().await;
    let check = d.view(ViewName::Check, json!({})).await;
    let html = d.ok("/endpoint?notices").await;
    let text = text_of(&panel(&html));
    if check["notices"].as_array().is_none_or(Vec::is_empty) {
        assert!(text.contains("No notices."), "{text}");
    } else {
        assert!(!text.contains("No notices."), "the empty home's own notices are listed: {text}");
    }
    assert!(text.contains(CHAT));
}
