//! The Providers page against the CLI for the same moment (T038, US4): sections and cards
//! against `providers` and `accounts list --long`; a provider's window against
//! `accounts list --long` and `model <provider> <model>`; the plugins panel against
//! `plugins list --community`.

use nullrouter_dashboard::page::ViewName;
use nullrouter_dashboard::pages::providers::{CUSTOM, SECTIONS, section_of};
use serde_json::{Value, json};

use crate::common::{Dash, assert_fields, assert_shows, text_of};

fn rows(v: &Value) -> &[Value] {
    v.as_array().expect("a list view").as_slice()
}

/// The plugin rows of `set`, with `status` when given.
fn of<'a>(plugins: &'a Value, set: &str, status: Option<&str>) -> Vec<&'a Value> {
    rows(plugins).iter().filter(|r| r["set"] == set && status.is_none_or(|s| r["status"] == s)).collect()
}

/// The provider card for `id`: its `<a>` element.
fn card_of(html: &str, id: &str) -> String {
    let at = html.find(&format!("href=\"/providers/{id}\"")).unwrap_or_else(|| panic!("no card for {id}"));
    let start = html[..at].rfind("<a ").expect("the card is a link");
    let end = at + html[at..].find("</a>").expect("the card closes");
    html[start..end].to_owned()
}

/// The `provider-grid` block whose heading is `title`.
fn section_html<'a>(html: &'a str, title: &str) -> &'a str {
    html.split("<div class=\"provider-grid\">")
        .skip(1)
        .find(|chunk| chunk.contains(&format!(">{title}</h2>")))
        .unwrap_or_else(|| panic!("no section {title:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn sections_and_cards_agree_with_providers_and_accounts() {
    let d = Dash::dashboard().await;
    let providers = d.view(ViewName::Providers, json!({})).await;
    let accounts = d.view(ViewName::Accounts, json!({})).await;
    let html = d.ok("/providers").await;

    // The section titles are 9router's, and the operator's own plugins sit under "Custom Providers".
    let titles: Vec<&str> = SECTIONS.iter().map(|(t, _)| *t).chain(std::iter::once(CUSTOM)).collect();
    for title in &titles {
        let wanted = rows(&providers).iter().any(|p| section_of(p) == *title);
        assert_eq!(html.contains(&format!(">{title}</h2>")), wanted || *title == CUSTOM, "{title}");
    }

    for p in rows(&providers) {
        let id = p["id"].as_str().unwrap();
        let section = section_html(&html, section_of(p));
        assert!(section.contains(&format!("href=\"/providers/{id}\"")), "{id} is under {}", section_of(p));
        assert_eq!(p["source"] == "user", section_of(p) == CUSTOM, "{id}: source and section");

        let text = text_of(&card_of(&html, id));
        assert!(text.contains(id), "{id}: {text}");
        let mine: Vec<&Value> = rows(&accounts).iter().filter(|a| a["provider"] == id).collect();
        if mine.is_empty() {
            assert!(text.contains("No connections"), "{id}: {text}");
            continue;
        }
        let n = mine.len();
        let count = format!("{n} {}", if n == 1 { "connection" } else { "connections" });
        assert!(text.contains(&count), "{id}: {count:?} in {text}");
        for a in &mine {
            let label = match a["state"].as_str().unwrap() {
                "needs_sign_in" => Some("needs sign-in"),
                "active" if a["state_text"].as_str().unwrap().starts_with("cooling") => Some("cooling"),
                s @ ("refused" | "refreshing" | "disabled") => Some(s),
                _ => None,
            };
            if let Some(label) = label {
                assert!(text.contains(label), "{id}: {label} in {text}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_agrees_with_accounts_list_and_model() {
    let d = Dash::dashboard().await;
    let providers = d.view(ViewName::Providers, json!({})).await;
    let mut models_seen = 0;
    let mut accounts_seen = 0;
    for p in rows(&providers) {
        let id = p["id"].as_str().unwrap();
        let html = d.ok(&format!("/providers/{id}")).await;

        // `accounts list --long`: the columns, the sign-in details, the command to sign in again.
        let accounts = d.view(ViewName::Accounts, json!({"provider": id})).await;
        for a in rows(&accounts) {
            accounts_seen += 1;
            let what = format!("{id}/{}", a["name"].as_str().unwrap());
            assert_fields(&html, a, &["name", "kind", "order", "secret", "state_text"], &what);
            if let Some(p) = a["priority"].as_f64() {
                assert!(text_of(&html).contains(&p.to_string()), "{what}: priority {p}");
            }
            if a["kind"] == "signin" {
                assert_fields(&html, a, &["email", "tier", "expires_at", "last_refresh_at"], &what);
            }
            if matches!(a["state"].as_str(), Some("needs_sign_in" | "refused")) {
                let run = format!("nullrouter accounts signin {id} {}", a["name"].as_str().unwrap());
                assert!(text_of(&html).contains(&run), "{what}: {run}");
            }
        }

        // `model <provider> <model>`, for every model the window lists.
        let list = d.view(ViewName::Model, json!({"provider": id})).await;
        let listed = rows(&list);
        for m in listed {
            models_seen += 1;
            let model = m["model"].as_str().unwrap();
            let cli = d.view(ViewName::Model, json!({"provider": id, "model": model})).await;
            assert_eq!(&cli, m, "{id}/{model}: the list is what `model <provider> <model>` prints");
            let keys = ["model", "name", "kind", "target_format", "supported_formats", "quota_family", "strip", "upstream_id"];
            assert_fields(&html, &cli, &keys, &format!("{id}/{model}"));
        }
        assert_eq!(html.matches("class=\"model-row\"").count(), listed.len(), "{id}: every model, none extra");
    }
    assert!(accounts_seen > 0 && models_seen > 0, "the fixture has accounts and models to compare");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_plugins_panel_agrees_with_plugins_list_community() {
    let d = Dash::dashboard().await;
    let plugins = d.view(ViewName::Plugins, json!({"community": true})).await;
    let html = d.ok("/providers").await;
    let start = html.find("<details class=\"side-panel\"").expect("the Provider plugins panel");
    let panel = &html[start..start + html[start..].find("</details>").unwrap()];
    let text = text_of(panel);
    assert!(text.contains("Provider plugins"), "{text}");

    let bundled = of(&plugins, "bundled", None);
    let installed = of(&plugins, "community", Some("installed"));
    let available = rows(&plugins).iter().filter(|r| r["set"] == "community" && r["status"] != "installed").count();
    assert!(text.contains(&format!("Bundled · {}", bundled.len())), "{text}");
    assert!(text.contains(&format!("Installed from community · {}", installed.len())), "{text}");
    assert!(text.contains(&format!("Community · not installed: {available} available.")), "{text}");
    assert!(text.contains("In the CLI: nullrouter plugins list --community"), "{text}");

    // Each bundled plugin: its source and its state.
    for r in bundled {
        let id = r["id"].as_str().unwrap();
        let row = panel
            .split("<div class=\"plugin-row\">")
            .skip(1)
            .find(|c| c.contains(&format!("title=\"{id}\"")))
            .unwrap_or_else(|| panic!("no row for {id}"));
        let row = text_of(row);
        assert!(row.contains("bundled") && row.contains(r["status"].as_str().unwrap()), "{id}: {row}");
        assert_shows(&html, &r["id"], "plugin id");
    }
}
