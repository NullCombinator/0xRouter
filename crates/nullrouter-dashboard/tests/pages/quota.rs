//! What Quota Tracker says in the cases User Story 2 names (T033): the account that needs signing
//! in again and the command that does it, the account waiting for its first poll or on a stale
//! one, pay-as-you-go with no bar, "Expiring first", and the empty home.
//!
//! The fixture home can't hold every state (a first poll that never came, a poll that failed after
//! a good one), so those cases hand the page the views a running server would answer with.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use nullrouter_dashboard::logos::Index;
use nullrouter_dashboard::page::{Fetched, Page, ViewName};
use nullrouter_dashboard::pages::{Ctx, Req, quota};
use nullrouter_server::views::View;
use serde_json::{Value, json};

use crate::common::{Dash, cards, text_of};

const START: &str = "2026-10-04T05:00:00.000Z";

fn fetched(view: ViewName, json: Value) -> Fetched {
    Fetched { view, args: json!({}), value: View::new(json), generation: 1 }
}

/// The Quota Tracker content for `query`, built from the given views.
fn render(query: &str, accounts: Value, quota_view: Value, routing: Value) -> String {
    let req = Req::parse("/quota", query).expect("/quota is a page");
    let page = Page {
        generation: 1,
        as_of: Timestamp::UNIX_EPOCH,
        attempts: 1,
        views: vec![
            fetched(ViewName::Accounts, accounts),
            fetched(ViewName::Quota, quota_view),
            fetched(ViewName::Routing, routing),
        ],
    };
    let (tz, logos) = (TimeZone::UTC, Index::default());
    let ctx = Ctx { req: &req, page: &page, tz: &tz, logos: &logos };
    match quota::body(&ctx) {
        Ok(body) => body.content.into_string(),
        Err(e) => panic!("the page failed: {}", e.message),
    }
}

fn account(provider: &str, name: &str) -> Value {
    json!({"provider": provider, "name": name, "kind": "key", "order": 1, "priority": 1.0, "secret": "…AAAA",
        "state": "active", "state_since": null, "state_reason": null, "state_text": "active",
        "email": null, "tier": null, "expires_at": null, "last_refresh_at": null})
}

fn polled(provider: &str, name: &str, latest: Value) -> Value {
    json!({"provider": provider, "name": name, "kind": "key", "reported": true, "interval_s": 600,
        "latest": latest, "last_failure": null})
}

fn unreported(provider: &str, name: &str) -> Value {
    json!({"provider": provider, "name": name, "kind": "key", "reported": false, "interval_s": 600,
        "latest": null, "last_failure": null})
}

/// One `routing` row of an account in a target.
fn route(provider: &str, name: &str, source: &str, extra: Value) -> Value {
    let tier = if source == "pay-as-you-go" { "payg" } else { "subscription" };
    let mut r = json!({"provider": provider, "account": name, "tier": tier, "source": source, "stale": false,
        "pending_first_poll": false, "polled_at": null, "priority": 1.0, "pace": 1.0, "share": 0.5, "deficit": 0,
        "cache_lifetime": "5m", "price_now": null, "why_not": null, "windows": []});
    for (k, v) in extra.as_object().expect("extra is an object") {
        r[k] = v.clone();
    }
    r
}

fn routing(rows: Vec<Value>) -> Value {
    let window = json!({"start": START, "length": "5h"});
    json!({"amortization": window, "targets": [{"target": "sonnet", "amortization_window": window, "accounts": rows}]})
}

fn window(name: &str, resets_at: &str) -> Value {
    json!({"name": name, "unit": "percent", "used": 10.0, "limit": null, "remaining": 90.0, "resets_at": resets_at})
}

/// An account that needs signing in again says so and names the command that does it; one that
/// doesn't, doesn't.
#[tokio::test(flavor = "multi_thread")]
async fn a_card_needing_sign_in_names_the_command() {
    let d = Dash::dashboard().await;
    let accounts = d.view(ViewName::Accounts, json!({"provider": null})).await;
    let html = d.ok("/quota").await;
    let all = cards(&html);
    let text_for = |p: &str, n: &str| {
        let anchor = format!("{p} {n} priority");
        all.iter().map(|c| text_of(c)).find(|t| t.contains(&anchor)).unwrap_or_else(|| panic!("no card for {p}/{n}"))
    };
    let mut needing = 0;
    for a in accounts.as_array().unwrap() {
        let (p, n) = (a["provider"].as_str().unwrap(), a["name"].as_str().unwrap());
        let text = text_for(p, n);
        let command = format!("nullrouter accounts signin {p} {n}");
        if a["state"] == "needs_sign_in" {
            needing += 1;
            assert!(text.contains("needs sign-in") && text.contains(&command), "{p}/{n}: {text}");
        } else {
            assert!(!text.contains("accounts signin"), "{p}/{n} does not need signing in: {text}");
        }
    }
    assert!(needing > 0, "the fixture home has an account that needs signing in");
}

/// An account still waiting for its first poll, or whose last poll failed, says so in `quota`'s
/// words; a healthy one says neither.
#[test]
fn pending_first_poll_and_stale_use_the_words_quota_uses() {
    let accounts = json!([account("anthropic", "new"), account("anthropic", "old"), account("anthropic", "fine")]);
    let at = "2026-10-04T09:50:00.000Z";
    let quota_view = json!([
        polled("anthropic", "new", Value::Null),
        polled("anthropic", "old", json!({"at": at, "windows": [window("weekly", "2026-10-09T09:00:00.000Z")]})),
        polled("anthropic", "fine", json!({"at": at, "windows": [window("weekly", "2026-10-09T09:00:00.000Z")]})),
    ]);
    let rows = |new: Value, old: Value| {
        routing(vec![
            route("anthropic", "new", "polled", new),
            route("anthropic", "old", "polled", old),
            route("anthropic", "fine", "polled", json!({})),
        ])
    };
    let html = render("", accounts, quota_view, rows(json!({"pending_first_poll": true}), json!({"stale": true})));
    let all = cards(&html);
    let text_for = |n: &str| {
        let anchor = format!("anthropic {n} priority");
        all.iter().map(|c| text_of(c)).find(|t| t.contains(&anchor)).unwrap_or_else(|| panic!("no card for {n}"))
    };
    let (new, old, fine) = (text_for("new"), text_for("old"), text_for("fine"));
    assert!(new.contains("pending first poll") && !new.contains("stale"), "{new}");
    assert!(old.contains("stale") && !old.contains("pending first poll"), "{old}");
    assert!(!fine.contains("stale") && !fine.contains("pending first poll"), "{fine}");
}

/// A pay-as-you-go account has no quota bar; an estimated one does.
#[test]
fn pay_as_you_go_has_no_bar() {
    let accounts = json!([account("openrouter", "main"), account("anthropic", "main")]);
    let quota_view = json!([unreported("openrouter", "main"), unreported("anthropic", "main")]);
    let counted = json!({"windows": [{"name": "5-hour", "unit": "weighted_tokens", "remaining_now": 6.0e6,
        "capacity": 12.0e6, "reserve": 0.05, "resets_at": "2026-10-04T10:00:00.000Z"}]});
    let html = render(
        "",
        accounts,
        quota_view,
        routing(vec![
            route("openrouter", "main", "pay-as-you-go", json!({"price_now": 3.0})),
            route("anthropic", "main", "estimated", counted),
        ]),
    );
    let all = cards(&html);
    let card_for = |p: &str| {
        let anchor = format!("{p} main priority");
        all.iter().find(|c| text_of(c).contains(&anchor)).unwrap_or_else(|| panic!("no card for {p}")).clone()
    };
    let payg = card_for("openrouter");
    assert!(text_of(&payg).contains("pay-as-you-go"), "{}", text_of(&payg));
    assert!(!payg.contains("role=\"meter\""), "no quota bar on a pay-as-you-go account: {payg}");
    let estimated = card_for("anthropic");
    assert!(estimated.contains("role=\"meter\""), "an estimated window has its bar: {estimated}");
    assert!(text_of(&estimated).contains("estimated") && text_of(&estimated).contains("6.0M/12.0M wtok"));
}

/// `order=expiring` puts the window that resets first first; without it the windows keep `quota`'s
/// order.
#[test]
fn expiring_first_sorts_windows_by_reset() {
    let latest = json!({"at": "2026-10-04T09:50:00.000Z", "windows": [
        window("weekly", "2026-10-09T09:00:00.000Z"),
        window("monthly", "2026-11-01T00:00:00.000Z"),
        window("5-hour", "2026-10-04T10:00:00.000Z"),
    ]});
    let page = |query: &str| {
        render(
            query,
            json!([account("anthropic", "main")]),
            json!([polled("anthropic", "main", latest.clone())]),
            routing(vec![route("anthropic", "main", "polled", json!({}))]),
        )
    };
    let at = |html: &str, name: &str| {
        html.find(&format!(">{name}<")).unwrap_or_else(|| panic!("{name} is not on the page"))
    };
    let plain = page("");
    assert!(at(&plain, "weekly") < at(&plain, "monthly") && at(&plain, "monthly") < at(&plain, "5-hour"));
    let sorted = page("order=expiring");
    assert!(at(&sorted, "5-hour") < at(&sorted, "weekly") && at(&sorted, "weekly") < at(&sorted, "monthly"));
    assert!(sorted.contains("checked"), "the toggle shows it is on");
}

/// Narrowing keeps its place in the form: the chosen provider and account are selected.
#[test]
fn the_filters_show_what_is_chosen() {
    let html = render(
        "provider=anthropic&account=main",
        json!([account("anthropic", "main"), account("xai", "work")]),
        json!([unreported("anthropic", "main")]),
        routing(vec![]),
    );
    assert!(html.contains("<option value=\"anthropic\" selected>"), "{html}");
    assert!(html.contains("<option value=\"main\" selected>"), "{html}");
    assert!(html.contains("<option value=\"xai\">"), "every provider is offered: {html}");
    assert!(html.contains("name=\"order\"") && html.contains("Expiring first"), "{html}");
}

/// A home with no accounts says how to add one.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_home_names_the_command_that_adds_an_account() {
    let d = Dash::empty().await;
    let page = text_of(&d.ok("/quota").await);
    assert!(page.contains("No accounts."), "{page}");
    assert!(page.contains("nullrouter accounts add <provider> <name>"), "{page}");
    assert!(!page.contains("Showing"), "no footer without accounts: {page}");
}
