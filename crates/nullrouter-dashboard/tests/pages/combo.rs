//! The Combo page (spec 014 US3): the forms that declare, change and remove unified models and
//! combos, the Test card, and the remove and test windows.

use nullrouter_dashboard::page::ViewName;
use nullrouter_engine::testkit::homes;
use nullrouter_server::ops::describe_loss;
use reqwest::StatusCode;
use reqwest::header::LOCATION;
use serde_json::json;

use crate::common::{Dash, Site, notice, text_of};

/// The first `<form>` posting to `action`, through its `</form>`.
fn form_to<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html.find(&format!("action=\"{action}\"")).unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").expect("the action belongs to a form");
    let end = at + html[at..].find("</form>").expect("the form closes");
    &html[start..end]
}

/// Combo lists its `combo` notices and no unified model list.
#[tokio::test(flavor = "multi_thread")]
async fn combo_lists_its_notices_and_no_unified_models() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    let page = text_of(&d.ok("/combo").await);
    let mine: Vec<&str> = check["notices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["subject"] == "combo")
        .map(|n| n["text"].as_str().unwrap())
        .collect();
    assert!(!mine.is_empty(), "the fixture has a dropped unified model or a limits note");
    for text in mine {
        let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(page.contains(&one_line), "{one_line:?} is on Combo\n{page}");
    }
    let unified = d.view(ViewName::Check, json!({})).await["unified_models"].clone();
    assert!(!page.contains("unified models:"), "no unified model list: {unified}");
}

/// Each declare form posts its name and its members, a textarea, one per line, and returns to the
/// page; no script runs.
#[tokio::test(flavor = "multi_thread")]
async fn the_declare_forms_take_a_name_and_members_in_a_textarea() {
    let d = Dash::dashboard().await;
    let html = d.ok("/combo").await;
    for route in ["unified/add", "unified/set", "combos/add", "combos/set"] {
        let form = form_to(&html, &format!("/do/{route}"));
        assert!(
            form.contains("name=\"_back\" value=\"/combo\"") && form.contains("name=\"_t\" value=\""),
            "{route}: {form}"
        );
        assert!(form.contains("name=\"name\""), "{route}: {form}");
        assert!(form.contains("<textarea") && form.contains("name=\"members\""), "{route}: {form}");
    }
    assert!(!html.contains("<script"), "no script");
}

/// Test All is a link to every pair, and a Test form names one target and opens the test window.
#[tokio::test(flavor = "multi_thread")]
async fn the_test_card_has_test_all_and_a_test_form_for_one_target() {
    let d = Dash::dashboard().await;
    let html = d.ok("/combo").await;
    assert!(html.contains("href=\"/combo?window=test&amp;target=all\""), "Test All links to every pair");
    assert!(text_of(&html).contains("Test All"));
    assert!(
        html.contains("name=\"window\" value=\"test\"") && html.contains("name=\"target\""),
        "a Test form for one target"
    );
}

/// The remove window of a unified model names what removing loses and posts the name; it returns to
/// the same name when refused. Nothing opens without a name.
#[tokio::test(flavor = "multi_thread")]
async fn remove_unified_opens_its_confirm_window_with_the_loss() {
    let d = Dash::dashboard().await;
    let html = d.ok("/combo?window=remove-unified&name=u").await;
    let loss = describe_loss("unified.remove", "u").unwrap();
    assert!(text_of(&html).contains(&loss), "{loss}\n{}", text_of(&html));
    let form = form_to(&html, "/do/unified/remove");
    assert!(
        form.contains("name=\"name\" value=\"u\"") && form.contains("name=\"_window\" value=\"remove-unified\""),
        "{form}"
    );
    assert!(form.contains("name=\"_back\" value=\"/combo?name=u\""), "a refusal reopens it: {form}");
    assert!(html.contains("class=\"modal\""), "a window");

    let nothing = d.ok("/combo?window=remove-unified").await;
    assert!(!nothing.contains("/do/unified/remove"), "no name, no window");
}

/// The remove window of a combo, as the unified one, with the combo's loss.
#[tokio::test(flavor = "multi_thread")]
async fn remove_combo_opens_its_confirm_window_with_the_loss() {
    let d = Dash::dashboard().await;
    let html = d.ok("/combo?window=remove-combo&name=c").await;
    let loss = describe_loss("combos.remove", "c").unwrap();
    assert!(text_of(&html).contains(&loss), "{loss}\n{}", text_of(&html));
    let form = form_to(&html, "/do/combos/remove");
    assert!(
        form.contains("name=\"name\" value=\"c\"") && form.contains("name=\"_window\" value=\"remove-combo\""),
        "{form}"
    );
    assert!(!d.ok("/combo?window=remove-combo").await.contains("/do/combos/remove"), "no name, no window");
}

/// Test All's window: the plan's question when the run bills more than one call (and its confirmed
/// answer), otherwise one click; either way it posts the target to the run.
#[tokio::test(flavor = "multi_thread")]
async fn test_all_shows_the_plan_and_posts_the_target_to_the_run() {
    let d = Dash::dashboard().await;
    let html = d.ok("/combo?window=test&target=all").await;
    let text = text_of(&html);
    let form = form_to(&html, "/do/test/run");
    assert!(form.contains("name=\"target\" value=\"all\""), "{form}");
    assert!(text.contains("Run test"), "{text}");
    let billed = text.contains("billed calls");
    assert_eq!(
        billed,
        form.contains("name=\"confirmed\" value=\"on\""),
        "only a run that bills more than one call is confirmed: {form}"
    );
    if billed {
        assert!(text.contains("Continue?"), "the CLI's question: {text}");
    }
}

/// Settings has the test-settings form (`verdicts settings`): a setting, a type, a value, and the
/// allowed settings as its hint; it returns to Settings.
#[tokio::test(flavor = "multi_thread")]
async fn settings_has_the_test_settings_form() {
    let d = Dash::dashboard().await;
    let html = d.ok("/settings").await;
    let form = form_to(&html, "/do/verdicts/settings");
    assert!(form.contains("name=\"_back\" value=\"/settings\""), "{form}");
    for field in ["name=\"setting\"", "name=\"type\"", "name=\"value\""] {
        assert!(form.contains(field), "{field}: {form}");
    }
    assert!(text_of(&html).contains("retest"), "the allowed settings are named");
}

/// A refused remove redirects to its own window, which opens again with the loss and the name.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_remove_reopens_its_window() {
    let site = Site::start_on(homes::dashboard()).await;
    let r = site
        .post(
            "unified/remove",
            "/combo?name=ghost",
            &[("_window", "remove-unified".to_string()), ("name", "ghost".to_string())],
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let location = r.headers()[LOCATION].to_str().unwrap().to_owned();
    assert!(location.contains("window=remove-unified") && location.contains("done="), "{location}");
    let (status, page) = site.get(&location).await;
    assert_eq!(status, StatusCode::OK, "{location}");
    assert!(notice(&page).is_some_and(|(refused, _)| refused), "the refusal is shown");
    assert!(page.contains("class=\"modal\""), "the window reopens");
    assert!(text_of(&page).contains(&describe_loss("unified.remove", "ghost").unwrap()));
}
