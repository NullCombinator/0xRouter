//! Console Log on a home with no dashboard changes shows its empty state (spec 014 US5). Proxy Pools is
//! built (spec 014 US2): its forms are checked here. Combo is built (spec 014 US3); see `combo.rs`.

use nullrouter_server::ops::describe_loss;

use crate::common::{Dash, text_of};

/// The first `<form>` posting to `action`, through its `</form>`.
fn form_to<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html.find(&format!("action=\"{action}\"")).unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").expect("the action belongs to a form");
    let end = at + html[at..].find("</form>").expect("the form closes");
    &html[start..end]
}

#[tokio::test(flavor = "multi_thread")]
async fn console_log_with_no_changes_says_so() {
    let d = Dash::dashboard().await;
    let log = d.ok("/console-log").await;
    assert!(text_of(&log).contains("No changes made from the dashboard yet."), "{log}");
    assert!(!log.contains("<table"), "no table without entries");
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

    for (action, field) in [
        ("/do/proxy/use", "name=\"proxy\""),
        ("/do/proxy/clear", "name=\"scope\""),
        ("/do/proxy/fixed", "name=\"name\""),
    ] {
        let form = form_to(&html, action);
        assert!(form.contains(field), "{action}: {form}");
        assert!(
            form.contains("name=\"_back\" value=\"/proxy-pools\"") && form.contains("name=\"_t\" value=\""),
            "{action}: {form}"
        );
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
    assert!(
        form.contains("name=\"name\" value=\"eu\"") && form.contains("name=\"_window\" value=\"remove\""),
        "{form}"
    );
    assert!(form.contains("name=\"_back\" value=\"/proxy-pools?name=eu\""), "a refusal reopens it: {form}");
    assert!(html.contains("class=\"modal\""), "a window");

    let nothing = d.ok("/proxy-pools?window=remove").await;
    assert!(!nothing.contains("/do/proxy/remove"), "no name, no window");
}
