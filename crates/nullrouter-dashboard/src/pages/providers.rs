//! Providers (contracts/dashboard-http.md "Providers"): the loaded providers in 9router's
//! sections, a window per provider, and the "Provider plugins" side panel. Every fact comes from
//! `providers`, `accounts list --long` and `plugins list --community`; the page adds none.
//!
//! The window shows each model with what `model <provider> <model>` shows: one `model` read in
//! list mode (`nullrouter model <provider>`) gives every model the provider declares.

use std::collections::BTreeSet;

use maud::{Markup, html};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::access::encode_component;
use crate::components::{
    self, Tone, badge, disabled_button, disabled_switch, empty, icon, kv, name, prose, side_row, side_section, status,
};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] =
    &[ViewName::Providers, ViewName::Accounts, ViewName::Plugins, ViewName::Model, ViewName::Latency, ViewName::Check];

/// The hints of research R15.
pub const ADD_HINT: &str = "Not built yet. Add a provider with a plugin file; see docs/plugins.md.";
pub const TEST_HINT: &str = "Not built yet: model tests come with their own slice.";
pub const SWITCH_HINT: &str = "Not built yet. Every loaded plugin is active.";
pub const INSTALL_HINT: &str = "In the CLI: `nullrouter plugins install <id>`";
pub const UNINSTALL_HINT: &str = "In the CLI: `nullrouter plugins uninstall <id>`";
pub const COMMUNITY_HINT: &str = "In the CLI: `nullrouter plugins list --community`.";

/// The operator's own plugins (`source` is `user`).
pub const CUSTOM: &str = "Custom Providers";

/// 9router's sections, in its order, with the `category` values each lists. 9router puts its free
/// and free-tier providers under one heading.
pub const SECTIONS: &[(&str, &[&str])] = &[
    ("OAuth Providers", &["oauth"]),
    ("Free Tier Providers", &["freeTier", "free"]),
    ("API Key Providers", &["apikey"]),
    ("Web Cookie Providers", &["webCookie"]),
];

pub fn wants(req: &Req) -> Vec<Want> {
    let mut wants = vec![
        Want::new(ViewName::Providers, json!({})),
        Want::new(ViewName::Accounts, json!({})),
        Want::new(ViewName::Plugins, json!({"community": true})),
        Want::new(ViewName::Check, json!({})),
    ];
    if let Some(id) = &req.window {
        wants.push(Want::new(ViewName::Model, json!({"provider": id})));
        wants.push(Want::new(ViewName::Latency, json!({})));
    }
    wants
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let providers = rows(ctx.json(ViewName::Providers));
    let accounts = rows(ctx.json(ViewName::Accounts));
    let window = match &ctx.req.window {
        Some(token) => Some(window(ctx, providers, accounts, token)?),
        None => None,
    };
    let q = ctx.req.get("q").map(str::to_lowercase);
    let has = ctx.req.get("has");
    let shown: Vec<&Value> = providers
        .iter()
        .filter(|p| q.as_deref().is_none_or(|q| matches(p, q)))
        .filter(|p| has.is_none_or(|k| rows(&p["capabilities"]).iter().any(|c| c == k)))
        .collect();

    let content = html! {
        (search(ctx.req, providers))
        (custom_section(ctx, &shown, accounts))
        @for (title, categories) in SECTIONS {
            @let mine: Vec<&Value> = shown.iter().copied().filter(|p| section_of(p) == *title && is_in(p, categories)).collect();
            @if !mine.is_empty() {
                (section(ctx, title, test_all(), &mine, accounts))
            }
        }
        @if shown.is_empty() && (q.is_some() || has.is_some()) {
            (empty("search", "No provider matches", "Search by provider id or alias."))
        }
    };
    Ok(Body { content, side: Some(side(ctx)), window })
}

/// The rows of `set` among the plugin rows.
fn of<'a>(plugins: &'a [Value], set: &'a str) -> impl Iterator<Item = &'a Value> {
    plugins.iter().filter(move |r| r["set"] == set)
}

fn rows(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], Vec::as_slice)
}

/// The section a provider is listed under: the operator's own plugins together, the rest by
/// category.
pub fn section_of(p: &Value) -> &'static str {
    if p["source"] == "user" {
        return CUSTOM;
    }
    let category = p["category"].as_str().unwrap_or_default();
    SECTIONS.iter().find(|(_, cats)| cats.contains(&category)).map_or("Other Providers", |(title, _)| title)
}

/// Whether the provider's category is one of `categories` (the section titles repeat for
/// `free` and `freeTier`; this keeps an unknown category out of every named section).
fn is_in(p: &Value, categories: &[&str]) -> bool {
    p["category"].as_str().is_some_and(|c| categories.contains(&c))
}

/// Whether `q` (lower case) is in the provider's id or alias.
fn matches(p: &Value, q: &str) -> bool {
    ["id", "alias"].iter().any(|k| p[*k].as_str().is_some_and(|s| s.to_lowercase().contains(q)))
}

/// The `q` search, a `GET` form (research R4) at the top right, as the mockup draws it: Enter
/// submits it.
fn search(req: &Req, providers: &[Value]) -> Markup {
    let has = req.get("has");
    html! {
        form class="provider-grid__search" method="get" action=(req.base()) role="search" {
            @if req.notices_open() { input type="hidden" name="notices" value=""; }
            @if let Some(k) = has { input type="hidden" name="has" value=(k); }
            label class="provider-grid__field" {
                (icon("search"))
                input class="provider-grid__input" type="search" name="q" value=(req.get("q").unwrap_or_default())
                    placeholder="Search providers..." aria-label="Search providers";
            }
            (kinds_menu(req, providers, has))
        }
    }
}

/// "All kinds": a menu of the model kinds the loaded plugins declare, as links; a provider is
/// listed under a kind when its `capabilities` (as `providers` prints them) include it.
fn kinds_menu(req: &Req, providers: &[Value], has: Option<&str>) -> Markup {
    let kinds: BTreeSet<&str> =
        providers.iter().flat_map(|p| rows(&p["capabilities"])).filter_map(Value::as_str).collect();
    let path = req.path();
    html! {
        details class="pick" {
            summary class="pick__button" { (has.unwrap_or("All kinds")) (icon("expand_more")) }
            div class="pick__menu" {
                a class=(if has.is_none() { "pick__item pick__item--current" } else { "pick__item" }) href=(req.href(&path, &["has"], &[])) { "All kinds" }
                @for kind in kinds {
                    a class=(if has == Some(kind) { "pick__item pick__item--current" } else { "pick__item" }) href=(req.href(&path, &[], &[("has", kind)])) { (kind) }
                }
            }
        }
    }
}

/// A section's "Test All", disabled, with its hint as the bar's note.
fn test_all() -> (Markup, Markup) {
    (prose(TEST_HINT), disabled_button("Test All", Some("play_arrow"), false))
}

/// "Custom Providers", always drawn: it holds the two disabled "Add … Compatible" controls.
fn custom_section(ctx: &Ctx<'_>, shown: &[&Value], accounts: &[Value]) -> Markup {
    let mine: Vec<&Value> = shown.iter().copied().filter(|p| section_of(p) == CUSTOM).collect();
    let controls = (prose(ADD_HINT), html! {
        (disabled_button("Add Anthropic Compatible", Some("add"), true))
        (disabled_button("Add OpenAI Compatible", Some("add"), false))
    });
    if mine.is_empty() {
        return html! {
            div class="provider-grid" {
                (components::section_bar_noted(CUSTOM, controls.0, controls.1))
                p class="provider-grid__none" { (icon("extension")) "No custom providers. Add one with a plugin file; see docs/plugins.md." }
            }
        };
    }
    section(ctx, CUSTOM, controls, &mine, accounts)
}

fn section(ctx: &Ctx<'_>, title: &str, controls: (Markup, Markup), providers: &[&Value], accounts: &[Value]) -> Markup {
    html! {
        div class="provider-grid" {
            (components::section_bar_noted(title, controls.0, controls.1))
            div class="provider-grid__cards" {
                @for p in providers { (card(ctx, p, &accounts_of(accounts, p["id"].as_str().unwrap_or_default()))) }
            }
        }
    }
}

fn accounts_of<'a>(accounts: &'a [Value], provider: &str) -> Vec<&'a Value> {
    accounts.iter().filter(|a| a["provider"] == provider).collect()
}

fn card(ctx: &Ctx<'_>, p: &Value, accounts: &[&Value]) -> Markup {
    let id = p["id"].as_str().unwrap_or_default();
    let alias = p["alias"].as_str().unwrap_or(id);
    let href = ctx.req.href(&format!("/providers/{}", encode_component(id)), &["kind"], &[]);
    let class =
        if ctx.req.window.as_deref() == Some(id) { "provider-card provider-card--open" } else { "provider-card" };
    html! {
        a class=(class) href=(href) {
            span class="provider-card__logo" { (components::logo(ctx.logos.href(id), alias)) }
            span class="provider-card__text" {
                b class="provider-card__id" { (name(id)) }
                span class="provider-card__accounts" { (accounts_line(accounts)) }
            }
        }
    }
}

/// An account's state when it isn't plain active, in the words the card shows.
fn state_label(a: &Value) -> Option<&'static str> {
    match a["state"].as_str()? {
        "active" => a["state_text"].as_str().is_some_and(|t| t.starts_with("cooling")).then_some("cooling"),
        "needs_sign_in" => Some("needs sign-in"),
        "refused" => Some("refused"),
        "refreshing" => Some("refreshing"),
        "disabled" => Some("disabled"),
        _ => None,
    }
}

/// `N connections`, then how many are in each state that isn't active; `No connections`.
fn accounts_line(accounts: &[&Value]) -> Markup {
    if accounts.is_empty() {
        return html! { "No connections" };
    }
    let mut tally: Vec<(&'static str, usize)> = Vec::new();
    for label in accounts.iter().filter_map(|a| state_label(a)) {
        match tally.iter_mut().find(|(l, _)| *l == label) {
            Some((_, n)) => *n += 1,
            None => tally.push((label, 1)),
        }
    }
    let n = accounts.len();
    html! {
        (n) (if n == 1 { " connection" } else { " connections" })
        @for (label, count) in &tally { " · " (count) " " (status(label)) }
    }
}

/// The window open on `/providers/<id>`: 404 with the CLI's words for an id (or alias) no
/// loaded provider has.
fn window(ctx: &Ctx<'_>, providers: &[Value], accounts: &[Value], token: &str) -> Result<Markup, Failure> {
    let p = providers
        .iter()
        .find(|p| p["id"] == token || p["alias"] == token)
        .ok_or_else(|| Failure::not_found(format!("not found: unknown provider {token:?}")))?;
    let req = ctx.req;
    let id = p["id"].as_str().unwrap_or_default();
    let alias = p["alias"].as_str().unwrap_or(id);
    let listed = ctx.json(ViewName::Model);
    if listed["kind"] == "not_found" {
        return Err(Failure::not_found(format!("not found: {}", listed["error"].as_str().unwrap_or_default())));
    }
    let models = rows(listed);
    let chosen = req.get("kind");
    let shown: Vec<&Value> = models.iter().filter(|m| chosen.is_none_or(|k| in_kind(m, k))).collect();
    let mine = accounts_of(accounts, id);

    let body = html! {
        div class="provider-window" {
            div class="provider-window__head" {
                span class="provider-window__logo" { (components::logo(ctx.logos.href(id), alias)) }
                div class="provider-window__titles" {
                    h3 class="provider-window__name" { (id) }
                    p class="provider-window__count" {
                        (accounts_line(&mine)) " · " (models.len()) (if models.len() == 1 { " model" } else { " models" })
                    }
                    div class="provider-window__facts" {
                        @if let Some(a) = p["alias"].as_str() { (kv("alias", html! { (a) })) }
                        (kv("category", html! { (p["category"].as_str().unwrap_or_default()) }))
                        (kv("source", html! { (p["source"].as_str().unwrap_or_default()) }))
                    }
                }
            }
            section class="provider-window__card" {
                div class="provider-window__card-head" {
                    h4 class="provider-window__heading" { "Connections" }
                    (last_response(ctx, id))
                }
                @if mine.is_empty() {
                    p class="provider-window__none" { "No connections" }
                } @else {
                    @for a in &mine { (account_row(ctx, a)) }
                }
            }
            section class="provider-window__card" {
                div class="provider-window__card-head" { h4 class="provider-window__heading" { "Available Models" } }
                (kind_filter(req, providers, models, chosen))
                @if shown.is_empty() {
                    p class="provider-window__none" {
                        @if models.is_empty() { "This provider declares no models." } @else { "No models of this kind." }
                    }
                } @else {
                    div class="provider-window__models" { @for m in &shown { (model_row(m)) } }
                }
            }
        }
    };
    let what = format!("provider plugin · {}", p["source"].as_str().unwrap_or_default());
    Ok(components::wide_modal(id, &what, &req.close_href(), body))
}

/// The provider's last response in the last 24 hours (`latency`'s `last`): "resolved · <time>",
/// "failed · <time> · <status>", or that there was none.
pub const NO_RESPONSE: &str = "None in the last 24 hours";

fn last_response(ctx: &Ctx<'_>, id: &str) -> Markup {
    let row = rows(&ctx.json(ViewName::Latency)["providers"]).iter().find(|r| r["id"] == id);
    let last = row.map(|r| &r["last"]).filter(|l| l["at"].is_string());
    html! {
        span class="provider-window__last" {
            "Last response "
            @if let Some(l) = last {
                (status(l["result"].as_str().unwrap_or_default())) " · " (ctx.time(l["at"].as_str().unwrap_or_default()))
                @if let Some(status) = l["status"].as_u64() { " · " (status) }
            } @else {
                (NO_RESPONSE)
            }
        }
    }
}

/// One account as `accounts list --long` shows it: the columns, then a sign-in account's email,
/// tier and token times, then the command that signs in again.
fn account_row(ctx: &Ctx<'_>, a: &Value) -> Markup {
    let text = |k: &str| a[k].as_str().unwrap_or("-").to_owned();
    let priority = a["priority"].as_f64().map_or_else(|| "-".to_owned(), |p| p.to_string());
    let when = |k: &str| match a[k].as_str() {
        Some(t) => ctx.time(t),
        None => html! { "-" },
    };
    let signin = a["kind"] == "signin";
    let command = format!("Run: `nullrouter accounts signin {} {}`", text("provider"), text("name"));
    html! {
        div class="provider-window__account" {
            span class="provider-window__account-icon" { (icon(if signin { "lock" } else { "key" })) }
            div class="provider-window__account-main" {
                b class="provider-window__account-name" { (name(&text("name"))) }
                div class="provider-window__account-badges" {
                    (status(&text("state_text")))
                    (badge(Tone::Default, &text("kind")))
                    span class="provider-window__account-order" { "order " (a["order"].to_string()) }
                }
                div class="provider-window__account-facts" {
                    (kv("priority", html! { (priority) }))
                    (kv("secret", html! { (text("secret")) }))
                    @if signin {
                        (kv("email", html! { (text("email")) }))
                        (kv("tier", html! { (text("tier")) }))
                        (kv("expires", when("expires_at")))
                        (kv("refreshed", when("last_refresh_at")))
                    }
                }
                @if matches!(a["state"].as_str(), Some("needs_sign_in" | "refused")) {
                    p class="provider-window__account-run" { (prose(&command)) }
                }
            }
        }
    }
}

/// `All`, then one link per kind any loaded plugin declares, each with this provider's count.
fn kind_filter(req: &Req, providers: &[Value], models: &[Value], chosen: Option<&str>) -> Markup {
    let kinds: BTreeSet<&str> =
        providers.iter().flat_map(|p| rows(&p["capabilities"])).filter_map(Value::as_str).collect();
    let path = req.path();
    let all = req.href(&path, &["kind"], &[]);
    html! {
        nav class="kind-filter" aria-label="Model kinds" {
            (kind_link(&all, "All", None, models.len(), chosen.is_none()))
            @for kind in kinds.iter().copied() {
                @let href = req.href(&path, &[], &[("kind", kind)]);
                @let count = models.iter().filter(|m| in_kind(m, kind)).count();
                (kind_link(&href, kind, Some(kind_icon(kind)), count, chosen == Some(kind)))
            }
        }
    }
}

/// `Registry::catalog`'s rule: a model is under `kind` when it has it, and untyped models are
/// listed under `llm`.
fn in_kind(m: &Value, kind: &str) -> bool {
    match m["kind"].as_str() {
        Some(k) => k == kind,
        None => kind == "llm",
    }
}

/// The icon of a model kind, as the mockups draw the kind chips.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "image" => "brush",
        "image_to_text" => "monitor",
        "video" => "movie",
        "tts" => "record_voice_over",
        "stt" => "mic",
        "embedding" => "data_array",
        "web_search" => "search",
        "web_fetch" => "link",
        "systemone" => "psychology",
        _ => "smart_toy",
    }
}

fn kind_link(href: &str, label: &str, icon_name: Option<&str>, count: usize, current: bool) -> Markup {
    let class = match (current, count) {
        (true, _) => "kind-filter__item kind-filter__item--current",
        (false, 0) => "kind-filter__item kind-filter__item--none",
        (false, _) => "kind-filter__item",
    };
    html! {
        a class=(class) href=(href) aria-current=[current.then_some("true")] {
            @if let Some(i) = icon_name { (icon(i)) }
            (label) " " span class="kind-filter__count" { (count) }
        }
    }
}

/// A model with what `model <provider> <model>` prints for it.
fn model_row(m: &Value) -> Markup {
    let list = |k: &str| m[k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
    let fact = |label: &str, value: Option<String>| html! { @if let Some(v) = value { (kv(label, html! { (v) })) } };
    let kind = m["kind"].as_str();
    html! {
        div class="model-row" {
            span class="model-row__icon" { (icon(kind_icon(kind.unwrap_or("llm")))) }
            div class="model-row__main" {
                @let id = m["model"].as_str().unwrap_or_default();
                code class="code model-row__id" title=(id) { (id) }
                div class="model-row__head" {
                    @if let Some(n) = m["name"].as_str() { i class="model-row__name" { (n) } }
                    @if let Some(k) = kind { (badge(Tone::Primary, k)) } @else { (badge(Tone::Default, "untyped")) }
                }
                div class="model-row__facts" {
                    (fact("declared", m["declared"].as_bool().map(|d| d.to_string())))
                    (fact("target format", m["target_format"].as_str().map(str::to_owned)))
                    (fact("supported formats", list("supported_formats")))
                    (fact("quota family", m["quota_family"].as_str().map(str::to_owned)))
                    (fact("strip", list("strip")))
                    (fact("upstream id", m["upstream_id"].as_str().map(str::to_owned)))
                }
            }
        }
    }
}

/// The id of the "Provider plugin settings" window: the checkbox the panel's gear toggles.
pub const SETTINGS: &str = "plugin-settings";

/// "Provider plugins": what `plugins list --community` lists, each row with its source, the
/// provider's category and its state; the gear opens the settings window.
fn side(ctx: &Ctx<'_>) -> Markup {
    let plugins = rows(ctx.json(ViewName::Plugins));
    let providers = rows(ctx.json(ViewName::Providers));
    let installed: Vec<&Value> = of(plugins, "community").filter(|r| r["status"] == "installed").collect();
    let not_installed = of(plugins, "community").filter(|r| r["status"] != "installed").count();
    let is_installed = |r: &Value| installed.iter().any(|i| i["id"] == r["id"]);
    let bundled: Vec<&Value> = of(plugins, "bundled").collect();
    let own: Vec<&Value> = of(plugins, "user").filter(|r| !is_installed(r)).collect();
    // An installed community plugin is loaded as a user plugin: that row says how it loaded.
    let state_of = |r: &Value| -> String {
        of(plugins, "user")
            .find(|u| u["id"] == r["id"])
            .and_then(|u| u["status"].as_str())
            .or_else(|| r["status"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let row = |r: &Value, state: &str| {
        let id = r["id"].as_str().unwrap_or_default();
        let set = r["set"].as_str().unwrap_or_default();
        let line = match providers.iter().find(|p| p["id"] == id).and_then(|p| p["category"].as_str()) {
            Some(category) => format!("{set} · {category}"),
            None => set.to_owned(),
        };
        side_row(components::logo(ctx.logos.href(id), id), id, &line, status(state))
    };
    let sections = html! {
        (side_section(&format!("Bundled · {}", bundled.len()), html! {
            @for r in &bundled { (row(r, r["status"].as_str().unwrap_or_default())) }
        }))
        (side_section(&format!("Installed from community · {}", installed.len()), html! {
            @for r in &installed { (row(r, &state_of(r))) }
        }))
        @if !own.is_empty() {
            (side_section(&format!("Your own · {}", own.len()), html! {
                @for r in &own { (row(r, r["status"].as_str().unwrap_or_default())) }
            }))
        }
        (side_section("Community · not installed", html! {
            p class="side-section__text" { (not_installed) " available. Open settings to install one." }
        }))
    };
    let listed: Vec<(&Value, bool)> = bundled
        .iter()
        .map(|r| (*r, false))
        .chain(installed.iter().map(|r| (*r, true)))
        .chain(own.iter().map(|r| (*r, false)))
        .collect();
    html! {
        (components::side_panel("extension", "Provider plugins", "Search plugins...", SETTINGS, sections))
        (components::settings(SETTINGS, "Provider plugin settings", settings_body(&listed, not_installed)))
    }
}

/// "Provider plugin settings": each plugin with its two switches (disabled) and, for a
/// community plugin, Uninstall; then the install row. Every control says where it lives today.
fn settings_body(listed: &[(&Value, bool)], available: usize) -> Markup {
    html! {
        table class="settings-table" {
            thead { tr { th { "Plugin" } th { "Source" } th { "Show on page" } th { "Enabled" } th {} } }
            tbody {
                @for (r, community) in listed {
                    @let id = r["id"].as_str().unwrap_or_default();
                    tr {
                        td { b { (name(id)) } }
                        td class="settings-table__muted" { (r["set"].as_str().unwrap_or_default()) }
                        td { (disabled_switch("Show on page", SWITCH_HINT)) }
                        td { (disabled_switch("Enabled", SWITCH_HINT)) }
                        td class="settings-table__action" {
                            @if *community {
                                span class="control" title=(UNINSTALL_HINT.replace("<id>", id).replace('`', "")) {
                                    (disabled_button("Uninstall", None, false))
                                }
                            }
                        }
                    }
                }
            }
        }
        p class="settings-table__note" { (prose(SWITCH_HINT)) " " (prose(UNINSTALL_HINT)) }
        (components::section_bar("Install a community plugin", html! {}))
        div class="settings-install" {
            label class="side-panel__search settings-install__search" title=(components::SEARCH_HINT) {
                (icon("search"))
                @let placeholder = format!("Search {available} community plugins...");
                input class="side-panel__input" type="search" placeholder=(placeholder) aria-label=(placeholder) disabled;
            }
            (disabled_button("Install", None, true))
        }
        p class="settings-table__note" { (prose(INSTALL_HINT)) " " (prose(COMMUNITY_HINT)) }
    }
}
