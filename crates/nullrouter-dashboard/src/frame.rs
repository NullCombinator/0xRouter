//! What every page sits in (contracts/dashboard-http.md "Frame"): 9router's sidebar under
//! "0Router Proxy", the page header with its "as of" line, the background grid, the page's own
//! `check` notices above its content, and the round housekeeping button with its panel.
//!
//! The panel is opened by its address (`?notices`, research R4): the button is a link to the same
//! page with `notices` toggled.

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use jiff::tz::TimeZone;
use maud::{DOCTYPE, Markup, html};
use serde_json::Value;

use crate::assets;
use crate::components::{self, icon, notice, prose};
use crate::logos::Index;
use crate::page::{Page, PageError, ViewName};
use crate::pages::{self, Body, Ctx, Id, Req};

/// The views the frame reads itself: `check` for the notices, `accounts` for the panel.
pub const VIEWS: &[ViewName] = &[ViewName::Check, ViewName::Accounts];

/// What the panel's chat box says.
pub const CHAT: &str = "Not built yet.";

/// One page's surroundings.
pub struct Frame<'a> {
    /// The marked sidebar entry; `None` on a page outside the sidebar.
    pub current: Option<Id>,
    pub version: &'a str,
    /// The header's "as of" line; `None` when no state was read.
    pub as_of: Option<String>,
    pub req: Option<&'a Req>,
    /// The `check` JSON, when it was read: the page's notices and the panel come from it.
    pub check: Option<&'a Value>,
    /// The `accounts` JSON, when the panel is open.
    pub accounts: Option<&'a Value>,
}

/// The whole document.
pub fn document(f: &Frame<'_>, body: Body) -> Markup {
    let (title, subtitle) = f.current.map_or(("0Router Proxy", ""), Id::title);
    let own: Vec<(&str, &str)> = match (f.current.and_then(Id::subject), f.check) {
        (Some(subject), Some(check)) => pages::notices_for(check, subject),
        _ => Vec::new(),
    };
    let panel_open = f.req.is_some_and(Req::notices_open);
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · 0Router Proxy" }
                @for sheet in assets::STYLESHEETS { link rel="stylesheet" href=(assets::href(sheet)); }
            }
            body class="frame" {
                div class="grid-bg" aria-hidden="true" {}
                (sidebar(f.current, f.version))
                main class="main" {
                    header class="page-header" {
                        @if let Some(id) = f.current { span class="page-header__icon" { (icon(id.icon())) } }
                        div class="page-header__titles" {
                            h1 class="page-header__title" { (title) }
                            @if !subtitle.is_empty() { p class="page-header__subtitle" { (subtitle) } }
                        }
                        @if let Some(as_of) = &f.as_of {
                            span class="page-header__as-of" { (icon("schedule")) (as_of) }
                        }
                    }
                    div class="main__column" {
                        @if !own.is_empty() {
                            div class="notices" { @for (level, text) in &own { (notice(level, text)) } }
                        }
                        (body.content)
                    }
                }
                @if let Some(side) = body.side { (side) }
                @if let Some(window) = body.window { (window) }
                @if panel_open { (panel(f.check, f.accounts, f.req)) }
                @if let Some(req) = f.req {
                    a class="floating-button" href=(req.toggle_notices_href(&req.path()))
                        title="Housekeeping: current notices" aria-label="Housekeeping: current notices" {
                        (icon("smart_toy"))
                    }
                }
            }
        }
    }
}

/// 9router's sidebar: the brand, the five entries, "System" and three more; the current entry is
/// marked and shows its filled icon.
fn sidebar(current: Option<Id>, version: &str) -> Markup {
    html! {
        aside class="sidebar" {
            div class="sidebar__brand" {
                span class="sidebar__mark" { (icon("hub")) }
                div class="sidebar__names" {
                    span class="sidebar__name" { "0Router Proxy" }
                    span class="sidebar__version" { (version) }
                }
            }
            nav class="sidebar__nav" aria-label="Pages" {
                @for id in Id::ALL {
                    @if id == Id::SYSTEM { p class="sidebar__heading" { "System" } }
                    @let on = current == Some(id);
                    a class=(if on { "nav-item nav-item--current" } else { "nav-item" }) href=(id.path())
                        aria-current=[on.then_some("page")] {
                        (icon(&if on { format!("{}_fill", id.icon()) } else { id.icon().to_owned() }))
                        span class="nav-item__label" { (id.label()) }
                    }
                }
            }
        }
    }
}

/// The housekeeping panel (FR-024, research R4): every `check` notice by level, then the accounts
/// that need the operator, then the chat box, disabled.
fn panel(check: Option<&Value>, accounts: Option<&Value>, req: Option<&Req>) -> Markup {
    let all: Vec<(&str, &str)> = check
        .and_then(|c| c["notices"].as_array())
        .into_iter()
        .flatten()
        .map(|n| (n["level"].as_str().unwrap_or("note"), n["text"].as_str().unwrap_or_default()))
        .collect();
    let needing: Vec<&Value> = accounts.and_then(Value::as_array).into_iter().flatten().filter(|a| needs_action(a)).collect();
    let close = req.map_or_else(|| "/".to_owned(), |r| r.toggle_notices_href(&r.path()));
    html! {
        section class="house-panel" aria-label="Housekeeping" {
            div class="house-panel__head" {
                (icon("smart_toy"))
                div class="house-panel__titles" {
                    b class="house-panel__title" { "Housekeeping" }
                    span class="house-panel__subtitle" { "Current notices from " (components::command("nullrouter check")) }
                }
                a class="house-panel__close" href=(close) aria-label="Close" { (icon("close")) }
            }
            div class="house-panel__body" {
                @if all.is_empty() && needing.is_empty() {
                    p class="house-panel__none" { "No notices." }
                }
                @for level in ["error", "warning", "note"] {
                    @for (l, text) in all.iter().filter(|(l, _)| *l == level) { (notice(l, text)) }
                }
                @if !needing.is_empty() {
                    h5 class="house-panel__heading" { "Accounts" }
                    @for a in &needing { (account_line(a)) }
                }
            }
            div class="house-panel__chat" {
                input class="input" type="text" disabled placeholder="Ask the housekeeping agent..." aria-label="Housekeeping chat";
                span class="hint" { (CHAT) }
            }
        }
    }
}

/// Needs sign-in, refused, refreshing, or resting in a cooldown.
fn needs_action(a: &Value) -> bool {
    let state = a["state"].as_str().unwrap_or_default();
    matches!(state, "needs_sign_in" | "refused" | "refreshing")
        || a["state_text"].as_str().is_some_and(|t| t.starts_with("cooling"))
}

/// `provider/name: <state as accounts list words it>`, and the command for a sign-in.
fn account_line(a: &Value) -> Markup {
    let (provider, name) = (a["provider"].as_str().unwrap_or_default(), a["name"].as_str().unwrap_or_default());
    let text = a["state_text"].as_str().unwrap_or_default();
    html! {
        div class="house-panel__account" {
            (components::status(text))
            span { b { (provider) "/" (name) } }
            @if a["state"] == "needs_sign_in" {
                span class="hint" { (prose(&format!("In the CLI: `nullrouter accounts signin {provider} {name}`"))) }
            }
        }
    }
}

/// The page for `req` from its built state, inside the frame.
pub fn render(req: &Req, page: &Page, version: &str, tz: &TimeZone, logos: &Index) -> Response {
    let ctx = Ctx { req, page, tz, logos };
    let (status, body) = match pages::body(&ctx) {
        Ok(body) => (StatusCode::OK, body),
        Err(f) => (f.status, Body::new(failure_card(&f.message))),
    };
    respond(status, &frame_for(req, page, version, tz), body)
}

/// The page under a window that couldn't open: the page as it is, and the window showing the
/// CLI's message (404).
pub fn render_missing_window(
    req: &Req,
    page: &Page,
    version: &str,
    tz: &TimeZone,
    logos: &Index,
    message: &str,
) -> Response {
    let ctx = Ctx { req, page, tz, logos };
    let mut body = pages::body(&ctx).unwrap_or_else(|f| Body::new(failure_card(&f.message)));
    body.window = Some(components::modal("Not found", &req.close_href(), html! { p { (message) } }));
    respond(StatusCode::NOT_FOUND, &frame_for(req, page, version, tz), body)
}

fn frame_for<'a>(req: &'a Req, page: &'a Page, version: &'a str, tz: &TimeZone) -> Frame<'a> {
    Frame {
        current: Some(req.id),
        version,
        as_of: Some(crate::time::as_of(page.as_of, tz)),
        req: Some(req),
        check: page.view(ViewName::Check).map(|v| &v.json),
        accounts: page.view(ViewName::Accounts).map(|v| &v.json),
    }
}

/// A page that could not be built at all (a view failed, the state kept changing, the build
/// panicked or ran out of time, or the dashboard is busy): the frame and the reason.
pub fn render_error(req: Option<&Req>, version: &str, status: StatusCode, text: &str) -> Response {
    let frame = Frame { current: req.map(|r| r.id), version, as_of: None, req, check: None, accounts: None };
    respond(status, &frame, Body::new(failure_card(text)))
}

/// The text of a page whose views failed.
pub fn build_failed(e: &PageError) -> String {
    format!("This page could not be built: {e}. Other pages and client requests are unaffected.")
}

fn failure_card(text: &str) -> Markup {
    components::card(None, html! { div class="failure" { (icon("error")) p { (text) } } })
}

fn respond(status: StatusCode, frame: &Frame<'_>, body: Body) -> Response {
    (status, Html(document(frame, body).into_string())).into_response()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn frame<'a>(req: &'a Req, check: &'a Value, accounts: &'a Value) -> Frame<'a> {
        Frame {
            current: Some(req.id),
            version: "nullrouter 0.1.0",
            as_of: Some("as of 15:04:05 CEST (Europe/Berlin) · reload to refresh".into()),
            req: Some(req),
            check: Some(check),
            accounts: Some(accounts),
        }
    }

    fn check() -> Value {
        json!({ "notices": [
            { "level": "warning", "subject": "quota", "text": "warning: xai/team: no window" },
            { "level": "error", "subject": "providers", "text": "skipped: bad.toml\n  line 3: oops" },
            { "level": "note", "subject": "settings", "text": "note: something" },
        ]})
    }

    #[test]
    fn the_sidebar_is_9routers_with_the_current_entry_marked() {
        let req = Req::parse("/quota", "").unwrap();
        let (c, a) = (check(), json!([]));
        let html = document(&frame(&req, &c, &a), Body::new(html! { p { "content" } })).into_string();
        let labels = ["Endpoint &amp; Key", "Providers", "Combo", "Usage", "Quota Tracker", "System", "Proxy Pools", "Console Log", "Settings"];
        let mut at = 0;
        for l in labels {
            let i = html[at..].find(l).unwrap_or_else(|| panic!("{l} after byte {at}: {html}"));
            at += i;
        }
        assert!(html.contains("0Router Proxy") && html.contains("nullrouter 0.1.0"));
        assert_eq!(html.matches("nav-item--current").count(), 1);
        assert!(html.contains(r#"class="nav-item nav-item--current" href="/quota" aria-current="page""#), "{html}");
        assert!(html.contains("as of 15:04:05 CEST (Europe/Berlin) · reload to refresh"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn a_page_shows_only_its_own_notices_and_the_button_toggles_the_panel() {
        let req = Req::parse("/quota", "").unwrap();
        let (c, a) = (check(), json!([]));
        let html = document(&frame(&req, &c, &a), Body::new(html! {})).into_string();
        assert!(html.contains("warning: xai/team: no window"));
        assert!(!html.contains("skipped: bad.toml"), "a providers notice stays off the quota page");
        assert!(html.contains(r#"class="floating-button" href="/quota?notices""#), "{html}");
        assert!(!html.contains("house-panel"));
    }

    #[test]
    fn the_panel_lists_every_notice_by_level_then_accounts_then_a_disabled_chat() {
        let req = Req::parse("/usage/records/rq_1", "notices").unwrap();
        let c = check();
        let a = json!([
            { "provider": "anthropic", "name": "work", "state": "needs_sign_in", "state_text": "needs sign-in since 2026-10-03 14:02 (refresh failed)" },
            { "provider": "xai", "name": "team", "state": "active", "state_text": "cooling grok-4 12 s" },
            { "provider": "xai", "name": "fine", "state": "active", "state_text": "active" },
        ]);
        let html = document(&frame(&req, &c, &a), Body::new(html! {})).into_string();
        let (e, w, n) = (html.find("skipped: bad.toml").unwrap(), html.find("warning: xai").unwrap(), html.find("note: something").unwrap());
        assert!(e < w && w < n, "errors, then warnings, then notes");
        assert!(html.contains("nullrouter accounts signin anthropic work"));
        assert!(html.contains("cooling grok-4 12 s") && !html.contains("xai/fine") && !html.contains(">fine<"));
        assert!(html.contains("disabled") && html.contains(CHAT));
        assert!(html.contains(r#"href="/usage/records/rq_1""#), "the button closes the panel on the same window: {html}");
    }

    #[test]
    fn an_empty_panel_says_so() {
        let req = Req::parse("/settings", "notices").unwrap();
        let (c, a) = (json!({ "notices": [] }), json!([]));
        let html = document(&frame(&req, &c, &a), Body::new(html! {})).into_string();
        assert!(html.contains("No notices."));
    }
}
