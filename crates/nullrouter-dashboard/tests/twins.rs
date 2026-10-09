//! Every fact on a page has a CLI twin (T070, SC-002, FR-003): a page fetches only the views its
//! module declares (plus `check` and `accounts` for the notices), and every word it shows, in text
//! or in a link, title, alt or time, is either the dashboard's own vocabulary (a word written in
//! its source) or found in those views' JSON. A word the page made up, or read from somewhere a
//! CLI command cannot reach, fails. Numbers and instants are left to the per-page agreement tests,
//! which compare them field by field; the header's "as of" is the clock, not the state.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use common::{Dash, attrs, text_of};
use nullrouter_dashboard::page::{self, ViewName};
use nullrouter_dashboard::pages::{self, Id, Req};
use nullrouter_engine::testkit::homes;
use serde_json::Value;

/// The attributes that carry what a page says; `class` and the like are markup.
const SAYING: [&str; 7] = ["href", "src", "alt", "title", "aria-label", "placeholder", "value"];

/// Lower-cased words of three letters or more with no digit in them.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !w.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_lowercase)
}

/// Every word written in the dashboard's source: labels, help text, icon names.
fn vocabulary() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.extend(words(&std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    out
}

/// Every word in a view's JSON, keys included.
fn json_words(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => out.extend(words(s)),
        Value::Array(a) => a.iter().for_each(|x| json_words(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                out.extend(words(k));
                json_words(x, out);
            }
        }
        _ => {}
    }
}

/// The page without the header's clock.
fn without_clock(html: &str) -> String {
    let Some(start) = html.find("class=\"page-header__as-of\"") else { return html.to_owned() };
    let end = html[start..].find("</span>").map_or(html.len(), |e| start + e);
    format!("{}{}", &html[..start], &html[end..])
}

/// What the page says: its text and the saying attributes, assets and logos left out.
fn said(html: &str) -> Vec<String> {
    let html = without_clock(html);
    let mut out = vec![text_of(&html)];
    for name in SAYING {
        out.extend(attrs(&html, name).into_iter().filter(|v| !v.starts_with("/assets/") && !v.starts_with("/logos/")));
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn every_word_on_a_page_is_the_dashboards_own_or_in_its_views() {
    let d = Dash::dashboard().await;
    let vocabulary = vocabulary();
    let mut paths: Vec<String> = Id::ALL.iter().map(|id| id.path().to_owned()).collect();
    paths.extend(Id::ALL.iter().map(|id| format!("{}?notices", id.path())));
    paths.push("/providers/anthropic".into());
    paths.push(format!("/usage/records/{}", homes::fallback_id()));

    let (mut checked, mut strays) = (0, Vec::new());
    for path in &paths {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let req = Req::parse(route, query).expect("a page route");
        let wants = pages::wants(&req);
        for w in &wants {
            assert!(
                req.id.views().contains(&w.view) || matches!(w.view, ViewName::Check | ViewName::Accounts),
                "{path}: fetches {:?}, which {:?} does not declare",
                w.view,
                req.id
            );
        }
        let built = page::build(&d.engine, &wants).await.expect("the page's views build");
        let mut known = BTreeSet::new();
        for f in &built.views {
            json_words(&f.value.json, &mut known);
            json_words(&f.value.extra, &mut known);
        }
        let html = d.ok(path).await;
        for text in said(&html) {
            for w in words(&text) {
                if vocabulary.contains(&w) {
                    continue;
                }
                if known.contains(&w) {
                    checked += 1;
                } else {
                    strays.push(format!("{path}: {w:?}"));
                }
            }
        }
    }
    strays.sort();
    strays.dedup();
    assert!(strays.is_empty(), "words in no declared view:\n{}", strays.join("\n"));
    assert!(checked > 0, "the fixture home's data words were found in the views");
}
