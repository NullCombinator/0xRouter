//! What the Providers page says in the cases its story names (T039, US4): the kind filter, the
//! disabled controls, a provider with no accounts, an unknown provider, the search.

use std::collections::BTreeSet;

use nullrouter_dashboard::page::ViewName;
use nullrouter_dashboard::pages::providers::{ADD_HINT, INSTALL_HINT, SWITCH_HINT, TEST_HINT};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Dash, text_of};

fn rows(v: &Value) -> &[Value] {
    v.as_array().expect("a list view").as_slice()
}

/// `Registry::catalog`'s rule: a model counts under `kind` when it has it; untyped ones under `llm`.
fn in_kind(m: &Value, kind: &str) -> bool {
    m["kind"].as_str().map_or(kind == "llm", |k| k == kind)
}

/// The kind filter's links as `(label, count, href)`.
fn filter(html: &str) -> Vec<(String, usize, String)> {
    let start = html.find("<nav class=\"kind-filter\"").expect("the window has a kind filter");
    let nav = &html[start..start + html[start..].find("</nav>").unwrap()];
    nav.split("<a class=\"kind-filter__item").skip(1).map(|a| {
        let href = a.split("href=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
        let text = text_of(a.split_once('>').unwrap().1);
        let (label, count) = text.rsplit_once(' ').expect("a label and a count");
        (label.to_owned(), count.parse().expect("the count is a number"), href)
    })
    .collect()
}

/// The model ids a window lists.
fn listed(html: &str) -> Vec<String> {
    html.split("model-row__id\">").skip(1).map(|c| c.split("</code>").next().unwrap().to_owned()).collect()
}

/// FR-031: the filter lists `All` and exactly the kinds the loaded plugins declare (their
/// capabilities), each with this provider's count; a kind no plugin declares isn't there.
#[tokio::test(flavor = "multi_thread")]
async fn the_kind_filter_lists_exactly_the_declared_kinds_with_counts() {
    let d = Dash::dashboard().await;
    let providers = d.view(ViewName::Providers, json!({})).await;
    let kinds: BTreeSet<&str> = rows(&providers)
        .iter()
        .flat_map(|p| p["capabilities"].as_array().map_or(&[][..], Vec::as_slice))
        .filter_map(Value::as_str)
        .collect();
    assert!(!kinds.is_empty(), "the fixture's plugins declare model kinds");
    assert!(!kinds.contains("decision"), "no plugin declares a decision model");

    for p in rows(&providers) {
        let id = p["id"].as_str().unwrap();
        let list = d.view(ViewName::Model, json!({"provider": id})).await;
        let models = rows(&list);
        let html = d.ok(&format!("/providers/{id}")).await;
        let links = filter(&html);

        let labels: Vec<&str> = links.iter().map(|(l, _, _)| l.as_str()).collect();
        let mut want = vec!["All"];
        want.extend(kinds.iter().copied());
        assert_eq!(labels, want, "{id}: All, then exactly the declared kinds");
        assert!(!labels.contains(&"decision"), "{id}");
        assert_eq!(links[0].1, models.len(), "{id}: All counts every model");
        assert_eq!(listed(&html).len(), models.len(), "{id}: All lists every model");

        for (label, count, href) in &links[1..] {
            let ids: Vec<&str> = models.iter().filter(|m| in_kind(m, label)).map(|m| m["model"].as_str().unwrap()).collect();
            assert_eq!(*count, ids.len(), "{id}: count of {label}");
            assert_eq!(href, &format!("/providers/{id}?kind={label}"), "{id}: link of {label}");

            let page = d.ok(href).await;
            let mut got = listed(&page);
            got.sort();
            let mut expected: Vec<String> = ids.iter().map(|s| (*s).to_owned()).collect();
            expected.sort();
            assert_eq!(got, expected, "{id}: ?kind={label} lists exactly those models");
        }
    }
}

/// `?kind=` keeps the other query values, and a kind nobody declares lists nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_kind_link_keeps_the_query_and_an_unknown_kind_lists_nothing() {
    let d = Dash::dashboard().await;
    let providers = d.view(ViewName::Providers, json!({})).await;
    let id = rows(&providers)[0]["id"].as_str().unwrap();
    let html = d.ok(&format!("/providers/{id}?q=zz&kind=nope")).await;
    assert!(listed(&html).is_empty(), "no model has the kind `nope`");
    assert!(text_of(&html).contains("No models of this kind."));
    for (label, _, href) in filter(&html) {
        assert!(href.contains("q=zz"), "{label}: {href}");
        assert!(label == "All" || href.ends_with(&format!("kind={label}")), "{label}: {href}");
    }
}

/// R15: each disabled control shows its hint, and none can submit anything.
#[tokio::test(flavor = "multi_thread")]
async fn every_disabled_control_shows_its_hint_and_has_no_form() {
    let d = Dash::dashboard().await;
    let html = d.ok("/providers").await;
    let text = text_of(&html);
    for label in ["Add Anthropic Compatible", "Add OpenAI Compatible", "Test All", "Install"] {
        assert!(text.contains(label), "{label}");
    }
    for hint in [ADD_HINT, TEST_HINT, SWITCH_HINT, INSTALL_HINT] {
        let plain = hint.replace('`', "");
        assert!(text.contains(&plain), "{plain:?} is on the page");
    }
    // Every button but the search's is disabled; the search is the page's only form, and a GET.
    for button in html.split("<button").skip(1) {
        let tag = button.split('>').next().unwrap();
        assert!(tag.contains("disabled") || tag.contains("type=\"submit\""), "{tag}");
    }
    assert_eq!(html.matches("<form").count(), 1, "only the search is a form");
    assert!(html.contains("<form class=\"provider-grid__search\" method=\"get\""));
    assert!(html.matches("role=\"switch\"").count() > 0 && html.matches("aria-disabled=\"true\"").count() > 0);
}

/// US4: a provider with no accounts says so, on its card and in its window.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_without_accounts_says_no_connections() {
    let d = Dash::dashboard().await;
    let providers = d.view(ViewName::Providers, json!({})).await;
    let accounts = d.view(ViewName::Accounts, json!({})).await;
    let bare: Vec<&str> = rows(&providers)
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .filter(|id| !rows(&accounts).iter().any(|a| a["provider"] == *id))
        .collect();
    assert!(!bare.is_empty(), "most bundled providers have no account in the fixture");

    let page = d.ok("/providers").await;
    let id = bare[0];
    let at = page.find(&format!("href=\"/providers/{id}\"")).unwrap();
    let card = text_of(&page[at..at + page[at..].find("</a>").unwrap()]);
    assert!(card.contains("No connections"), "{card}");

    let window = text_of(&d.ok(&format!("/providers/{id}")).await);
    assert!(window.contains("Accounts No connections"), "{window}");
}

/// A provider that has accounts shows the count and the states that aren't active.
#[tokio::test(flavor = "multi_thread")]
async fn a_card_counts_connections_and_names_the_states_that_are_not_active() {
    let d = Dash::dashboard().await;
    let page = d.ok("/providers").await;
    let at = page.find("href=\"/providers/anthropic\"").unwrap();
    let card = text_of(&page[at..at + page[at..].find("</a>").unwrap()]);
    assert!(card.contains("2 connections") && card.contains("1 disabled"), "{card}");
}

/// A window for a provider no plugin declares is a 404 in the frame, in the CLI's words.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_provider_is_a_404_with_the_clis_message() {
    let d = Dash::dashboard().await;
    let (status, html) = d.page("/providers/no-such-provider").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let text = text_of(&html);
    assert!(text.contains("unknown provider \"no-such-provider\""), "{text}");
    assert!(text.contains("Providers"), "inside the frame");
}

/// The search is a GET form that filters by id or alias.
#[tokio::test(flavor = "multi_thread")]
async fn the_search_filters_cards_by_id() {
    let d = Dash::dashboard().await;
    let page = d.ok("/providers?q=ANTHRO").await;
    // A card's link keeps the query, so only the start of the address is fixed.
    assert!(page.contains("href=\"/providers/anthropic"), "{page}");
    assert!(!page.contains("href=\"/providers/xai"));
    let none = text_of(&d.ok("/providers?q=zzzz-nothing").await);
    assert!(none.contains("No provider matches"), "{none}");
}
