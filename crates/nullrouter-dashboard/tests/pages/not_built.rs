//! Combo and Console Log say what isn't built and what exists instead (T042, research R15). Proxy
//! Pools is built (spec 014 US2): its forms are checked here.

use nullrouter_dashboard::pages::{combo, console_log};
use nullrouter_dashboard::page::ViewName;
use nullrouter_server::ops::describe_loss;
use serde_json::json;

use crate::common::{Dash, text_of};

/// The first `<form>` posting to `action`, through its `</form>`.
fn form_to<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html.find(&format!("action=\"{action}\"")).unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").expect("the action belongs to a form");
    let end = at + html[at..].find("</form>").expect("the form closes");
    &html[start..end]
}

#[tokio::test(flavor = "multi_thread")]
async fn each_not_built_entry_says_so_and_names_what_exists() {
    let d = Dash::dashboard().await;
    let combo_page = text_of(&d.ok("/combo").await);
    assert!(combo_page.contains(combo::TITLE), "{combo_page}");
    assert!(combo_page.contains("[[unified_model]]") && combo_page.contains("nullrouter unified"), "{combo_page}");

    let log = d.ok("/console-log").await;
    assert!(text_of(&log).contains(console_log::TITLE) && text_of(&log).contains(console_log::HINT));
    assert!(log.contains("href=\"/usage\""), "Console Log links to the Usage page");
}

/// Proxy Pools: add has a write-only password, use, clear and probe post to their routes with the
/// token and return to the page, and no script runs.
#[tokio::test(flavor = "multi_thread")]
async fn proxy_pools_forms_post_to_their_routes_and_the_password_is_write_only() {
    let d = Dash::dashboard().await;
    let html = d.ok("/proxy-pools").await;

    let add = form_to(&html, "/do/proxy/add");
    assert!(add.contains("name=\"_back\" value=\"/proxy-pools\"") && add.contains("name=\"_t\" value=\""), "{add}");
    for field in ["name=\"name\"", "name=\"url\"", "name=\"username\""] {
        assert!(add.contains(field), "{field}: {add}");
    }
    let at = add.find("name=\"password\"").expect("the add form has a password");
    let tag = &add[add[..at].rfind('<').unwrap()..at + add[at..].find('>').unwrap()];
    assert!(tag.contains("type=\"password\"") && !tag.contains("value"), "a write-only field has no value: {tag}");

    for (action, field) in [("/do/proxy/use", "name=\"proxy\""), ("/do/proxy/clear", "name=\"scope\""), ("/do/proxy/fixed", "name=\"name\"")] {
        let form = form_to(&html, action);
        assert!(form.contains(field), "{action}: {form}");
        assert!(form.contains("name=\"_back\" value=\"/proxy-pools\"") && form.contains("name=\"_t\" value=\""), "{action}: {form}");
    }
    assert!(!html.contains("<script"), "no script");
}

/// Remove is a link-like GET that opens a confirm window naming what is lost; its form posts the
/// name, and nothing opens without one.
#[tokio::test(flavor = "multi_thread")]
async fn proxy_remove_opens_a_confirm_window_with_the_loss() {
    let d = Dash::dashboard().await;
    let html = d.ok("/proxy-pools?window=remove&name=eu").await;
    let loss = describe_loss("proxy.remove", "eu").unwrap();
    assert!(text_of(&html).contains(&loss), "{loss}\n{}", text_of(&html));
    let form = form_to(&html, "/do/proxy/remove");
    assert!(form.contains("name=\"name\" value=\"eu\"") && form.contains("name=\"_window\" value=\"remove\""), "{form}");
    assert!(form.contains("name=\"_back\" value=\"/proxy-pools?name=eu\""), "a refusal reopens it: {form}");
    assert!(html.contains("class=\"modal\""), "a window");

    let nothing = d.ok("/proxy-pools?window=remove").await;
    assert!(!nothing.contains("/do/proxy/remove"), "no name, no window");
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
