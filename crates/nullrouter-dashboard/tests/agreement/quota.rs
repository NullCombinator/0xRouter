//! Quota Tracker against `accounts list --long`, `quota` and `routing` (T032, SC-001): every
//! scalar of a fact the page shows is on the page, instants through `<time datetime>`. Numbers are
//! compared as the CLI prints them (`quota_text`, `routing_text`), so the formats are written out
//! here once, independently of the page's.

use nullrouter_dashboard::page::ViewName;
use serde_json::{Value, json};

use crate::common::{Dash, assert_fields, assert_shows, cards, text_of};

/// `1,240`, `87.5`, `0`.
fn num(x: f64) -> String {
    let x = (x.abs() * 10.0).round() / 10.0;
    let whole = (x.trunc() as u64).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.insert(0, ',');
        }
        grouped.insert(0, c);
    }
    let tenth = ((x - x.trunc()) * 10.0).round() as u64;
    if tenth == 0 { grouped } else { format!("{grouped}.{tenth}") }
}

/// `10 min`, `1 h`, `90 s`.
fn every(secs: u64) -> String {
    if secs != 0 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs != 0 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// `5.6M`, `91.2k`, `250`.
fn si(x: f64) -> String {
    if x.abs() >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else if x.abs() >= 1e3 {
        format!("{:.1}k", x / 1e3)
    } else {
        format!("{x:.0}")
    }
}

fn deficit(n: i64) -> String {
    match n {
        0 => "0".into(),
        n if n > 0 => format!("+{}", si(n as f64)),
        n => si(n as f64),
    }
}

/// `1` for `1.0`, `0.5` for `0.5`.
fn plain(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x}") }
}

/// What `quota` prints for one polled window's value.
fn figure(w: &Value) -> String {
    let f = |k: &str| w[k].as_f64();
    let unit = w["unit"].as_str().unwrap_or("percent");
    if unit == "percent" {
        let left = f("remaining").or(f("used").map(|u| (100.0 - u).max(0.0))).unwrap_or(0.0);
        return format!("{}% left", num(left));
    }
    match (f("used"), f("limit"), f("remaining")) {
        (Some(u), Some(l), _) => format!("{} / {} {unit} used", num(u), num(l)),
        (Some(u), None, _) => format!("{} {unit} used", num(u)),
        (None, _, Some(r)) => format!("{} {unit} left", num(r)),
        _ => "not reported".into(),
    }
}

/// The HTML of the card of `provider/name`: the one whose header reads `<provider> <name> priority`.
fn card_of(html: &str, provider: &str, name: &str) -> String {
    let anchor = format!("{provider} {name} priority");
    cards(html)
        .into_iter()
        .find(|c| text_of(c).contains(&anchor))
        .unwrap_or_else(|| panic!("no card for {provider}/{name}\n{}", text_of(html)))
}

fn list(v: &Value) -> &[Value] {
    v.as_array().expect("the view is a list")
}

/// Every fact `accounts list --long`, `quota` and `routing --json` hold for an account is on its card.
#[tokio::test(flavor = "multi_thread")]
async fn quota_tracker_agrees_with_accounts_quota_and_routing() {
    let d = Dash::dashboard().await;
    let html = d.ok("/quota").await;
    let accounts = d.view(ViewName::Accounts, json!({"provider": null})).await;
    let quota = d.view(ViewName::Quota, json!({"provider": null, "name": null})).await;
    let routing = d.view(ViewName::Routing, json!({"target": null})).await;
    let (accounts, quota) = (list(&accounts), list(&quota));
    assert!(accounts.len() >= 6, "the fixture home has every status: {}", accounts.len());
    assert_eq!(quota.len(), accounts.len());

    let page = text_of(&html);
    assert!(
        page.contains(&format!("Showing {} accounts · polled quota is read from the provider", accounts.len())),
        "{page}"
    );
    assert!(page.contains("estimated quota is counted from your requests."), "{page}");

    // The amortization window `routing` shows is in the header.
    assert_shows(&html, &routing["amortization"]["length"], "amortization.length");
    assert_shows(&html, &routing["amortization"]["start"], "amortization.start");

    // `accounts list --long`.
    for a in accounts {
        let who = format!("{}/{}", a["provider"].as_str().unwrap(), a["name"].as_str().unwrap());
        let card = card_of(&html, a["provider"].as_str().unwrap(), a["name"].as_str().unwrap());
        let text = text_of(&card);
        assert_fields(&card, a, &["provider", "name", "kind", "order"], &who);
        assert!(text.contains(&format!("priority {}", plain(a["priority"].as_f64().unwrap()))), "{who}: {text}");
        match a["state"].as_str().unwrap() {
            "needs_sign_in" | "refused" => {
                assert_fields(&card, a, &["state_since", "state_reason"], &who);
                let label = if a["state"] == "refused" { "refused by provider" } else { "needs sign-in" };
                assert!(text.contains(label), "{who}: {text}");
            }
            _ => assert_shows(&card, &a["state_text"], &format!("{who}.state_text")),
        }
        if a["kind"] == "signin" {
            assert_fields(&card, a, &["email", "tier", "expires_at", "last_refresh_at"], &who);
        }
    }

    // `quota`.
    for q in quota {
        let who = format!("{}/{}", q["provider"].as_str().unwrap(), q["name"].as_str().unwrap());
        let card = card_of(&html, q["provider"].as_str().unwrap(), q["name"].as_str().unwrap());
        if q["reported"] != true {
            continue;
        }
        if q["latest"].is_object() {
            assert_shows(&card, &q["latest"]["at"], &format!("{who}.latest.at"));
            let every = every(q["interval_s"].as_u64().unwrap());
            assert!(text_of(&card).contains(&format!("every {every}")), "{who}: every {every}");
            for w in list(&q["latest"]["windows"]) {
                assert_fields(&card, w, &["name", "resets_at"], &format!("{who}.window"));
                assert_shows(&card, &json!(figure(w)), &format!("{who}.{} figure", w["name"]));
            }
        }
        if q["last_failure"].is_object() {
            assert_shows(&card, &q["last_failure"]["at"], &format!("{who}.last_failure.at"));
            assert_shows(&card, &q["last_failure"]["error"]["summary"], &format!("{who}.last_failure.summary"));
        }
    }

    // `routing`: pace, share and deficit of each account in each target it serves.
    let mut rows = 0;
    for t in list(&routing["targets"]) {
        for r in list(&t["accounts"]) {
            rows += 1;
            let (provider, name) = (r["provider"].as_str().unwrap(), r["account"].as_str().unwrap());
            let who = format!("{provider}/{name} in {}", t["target"]);
            let card = card_of(&html, provider, name);
            let text = text_of(&card);
            assert_shows(&card, &t["target"], &who);
            assert_shows(&card, &r["source"], &format!("{who}.source"));
            let pace = r["pace"].as_f64().map_or("—".to_owned(), |p| format!("{p:.2}"));
            let share = r["share"].as_f64().map_or("—".to_owned(), |s| format!("{:.0}%", s * 100.0));
            assert!(text.contains(&format!("pace {pace}")), "{who}: {text}");
            assert!(text.contains(&format!("share {share}")), "{who}: {text}");
            assert!(text.contains(&format!("deficit {}", deficit(r["deficit"].as_i64().unwrap()))), "{who}: {text}");
            if r["pending_first_poll"] == true {
                assert!(text.contains("pending first poll"), "{who}: {text}");
            }
            if r["stale"] == true {
                assert!(text.contains("stale"), "{who}: {text}");
            }
            // An account with no poll to show has the windows `routing` counts.
            let q = quota.iter().find(|q| q["provider"] == r["provider"] && q["name"] == r["account"]).unwrap();
            let has_poll = q["latest"]["windows"].as_array().is_some_and(|w| !w.is_empty());
            if r["tier"] != "payg" && !has_poll {
                for w in list(&r["windows"]) {
                    assert_fields(&card, w, &["name", "resets_at"], &format!("{who}.window"));
                    let unit = if w["unit"] == "requests" { "req" } else { "wtok" };
                    let want = format!(
                        "{}/{} {unit}",
                        si(w["remaining_now"].as_f64().unwrap()),
                        si(w["capacity"].as_f64().unwrap())
                    );
                    assert!(text.contains(&want), "{who}: {want:?} is not on the card\n{text}");
                }
            }
        }
    }
    assert!(rows > 0, "the fixture home routes at least one target: {routing}");
}

/// `?provider=…&account=…` shows exactly the accounts `quota <provider> <name>` shows.
#[tokio::test(flavor = "multi_thread")]
async fn narrowing_shows_exactly_the_accounts_quota_shows() {
    let d = Dash::dashboard().await;
    let all = d.view(ViewName::Quota, json!({"provider": null, "name": null})).await;
    let all = list(&all);

    let one = d.view(ViewName::Quota, json!({"provider": "xai", "name": "work"})).await;
    assert_eq!(list(&one).len(), 1);
    let html = d.ok("/quota?provider=xai&account=work").await;
    let page = text_of(&html);
    assert!(page.contains("Showing 1 accounts"), "{page}");
    let card = card_of(&html, "xai", "work");
    assert_fields(&card, &list(&one)[0], &["provider", "name"], "xai/work");
    for q in all.iter().filter(|q| !(q["provider"] == "xai" && q["name"] == "work")) {
        let anchor = format!("{} {} priority", q["provider"].as_str().unwrap(), q["name"].as_str().unwrap());
        assert!(cards(&html).iter().all(|c| !text_of(c).contains(&anchor)), "{anchor} is not narrowed away");
    }

    let provider = d.view(ViewName::Quota, json!({"provider": "xai", "name": null})).await;
    let html = d.ok("/quota?provider=xai").await;
    assert!(text_of(&html).contains(&format!("Showing {} accounts", list(&provider).len())));
    for q in list(&provider) {
        card_of(&html, "xai", q["name"].as_str().unwrap());
    }
    assert!(list(&provider).len() < all.len(), "other providers are narrowed away");
}
