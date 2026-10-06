//! Nothing leaves the machine (T056, FR-015): every page, window and the sign-in page refer only
//! to `/assets/…`, `/logos/…` and the dashboard's own routes; no page has a script; no `src`,
//! `href` or stylesheet `url()` names an `http(s)://` address. The endpoint card's URL is text,
//! not a link, so it is the one `http://` a page may show.

mod common;

use common::{Dash, attrs};
use nullrouter_dashboard::assets;
use nullrouter_dashboard::pages::Id;
use nullrouter_dashboard::pages::signin::{self, Screen};
use nullrouter_engine::testkit::homes;

/// Every address a page may refer to: an asset, a logo, or a dashboard route.
fn local(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    path.starts_with("/assets/")
        || path.starts_with("/logos/")
        || path == "/"
        || path == "/signin"
        || Id::ALL.iter().any(|id| path == id.path() || path.starts_with(&format!("{}/", id.path())))
}

fn check(path: &str, html: &str) {
    let lower = html.to_ascii_lowercase();
    assert!(!lower.contains("<script"), "{path}: a script");
    assert!(!event_handler(&lower), "{path}: an inline event handler");
    for name in ["src", "href", "action"] {
        for url in attrs(html, name) {
            assert!(local(&url), "{path}: {name}={url:?} is not a dashboard address");
        }
    }
}

/// `onclick=`, `onload=` and the like.
fn event_handler(html: &str) -> bool {
    html.match_indices(" on").any(|(i, _)| {
        let rest = &html[i + 3..];
        let name: String = rest.chars().take_while(char::is_ascii_lowercase).collect();
        !name.is_empty() && rest[name.len()..].starts_with('=')
    })
}

#[tokio::test]
async fn every_page_and_window_refers_only_to_the_dashboard() {
    let d = Dash::dashboard().await;
    let mut paths: Vec<String> = Id::ALL.iter().map(|id| id.path().to_owned()).collect();
    paths.extend(Id::ALL.iter().map(|id| format!("{}?notices", id.path())));
    paths.push("/providers/anthropic".into());
    paths.push(format!("/usage/records/{}", homes::fallback_id()));
    for path in &paths {
        let html = d.ok(path).await;
        check(path, &html);
    }
}

#[test]
fn the_sign_in_page_refers_only_to_the_dashboard() {
    for screen in [Screen::NoToken, Screen::Form { next: "/quota".into(), wrong: true }] {
        check("/signin", &signin::page(&screen).into_string());
    }
}

#[test]
fn no_stylesheet_names_an_outside_address() {
    for sheet in assets::STYLESHEETS {
        let css = std::fs::read_to_string(format!("{}/style/{sheet}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        for (i, _) in css.match_indices("url(") {
            let arg = css[i + 4..].split(')').next().unwrap().trim_matches(['"', '\'', ' ']);
            assert!(!arg.contains("://") && !arg.starts_with("//"), "{sheet}: url({arg})");
        }
        assert!(!css.contains("@import"), "{sheet}: @import");
    }
}
