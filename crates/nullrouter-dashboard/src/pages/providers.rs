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
use crate::components::{self, Tone, badge, disabled, disabled_switch, empty, icon, kv, modal, name, prose, slot, status};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Providers, ViewName::Accounts, ViewName::Plugins, ViewName::Model, ViewName::Check];

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
    let shown: Vec<&Value> = providers.iter().filter(|p| q.as_deref().is_none_or(|q| matches(p, q))).collect();

    let content = html! {
        (search(ctx.req))
        (custom_section(ctx, &shown, accounts))
        @for (title, categories) in SECTIONS {
            @let mine: Vec<&Value> = shown.iter().copied().filter(|p| section_of(p) == *title && is_in(p, categories)).collect();
            @if !mine.is_empty() {
                (section(ctx, title, disabled("Test All", None, TEST_HINT), &mine, accounts))
            }
        }
        @if shown.is_empty() && q.is_some() {
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

/// The `q` search, a `GET` form (research R4).
fn search(req: &Req) -> Markup {
    html! {
        form class="provider-grid__search" method="get" action=(req.base()) role="search" {
            @if req.notices_open() { input type="hidden" name="notices" value=""; }
            (icon("search"))
            input class="input" type="search" name="q" value=(req.get("q").unwrap_or_default())
                placeholder="Search providers..." aria-label="Search providers";
            button class="button button--secondary" type="submit" { "Search" }
        }
    }
}

/// "Custom Providers", always drawn: it holds the two disabled "Add … Compatible" controls.
fn custom_section(ctx: &Ctx<'_>, shown: &[&Value], accounts: &[Value]) -> Markup {
    let mine: Vec<&Value> = shown.iter().copied().filter(|p| section_of(p) == CUSTOM).collect();
    let controls = html! {
        (disabled("Add Anthropic Compatible", Some("add"), ADD_HINT))
        (disabled("Add OpenAI Compatible", Some("add"), ADD_HINT))
    };
    if mine.is_empty() {
        return html! {
            div class="provider-grid" {
                (components::section_bar(CUSTOM, controls))
                (empty("extension", "No custom providers", "Add one with a plugin file; see docs/plugins.md."))
            }
        };
    }
    section(ctx, CUSTOM, controls, &mine, accounts)
}

fn section(ctx: &Ctx<'_>, title: &str, controls: Markup, providers: &[&Value], accounts: &[Value]) -> Markup {
    html! {
        div class="provider-grid" {
            (components::section_bar(title, controls))
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
    let class = if ctx.req.window.as_deref() == Some(id) { "provider-card provider-card--open" } else { "provider-card" };
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
                    div class="provider-window__facts" {
                        @if let Some(a) = p["alias"].as_str() { (kv("alias", html! { (a) })) }
                        (kv("category", html! { (p["category"].as_str().unwrap_or_default()) }))
                        (kv("source", html! { (p["source"].as_str().unwrap_or_default()) }))
                    }
                }
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Accounts" }
                @if mine.is_empty() {
                    p class="provider-window__none" { "No connections" }
                } @else {
                    @for a in &mine { (account_row(ctx, a)) }
                }
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Models" }
                (kind_filter(req, providers, models, chosen))
                @if shown.is_empty() {
                    p class="provider-window__none" {
                        @if models.is_empty() { "This provider declares no models." } @else { "No models of this kind." }
                    }
                } @else {
                    div class="provider-window__models" { @for m in &shown { (model_row(m)) } }
                }
            }
            (slot("Last response"))
        }
    };
    Ok(modal(id, &req.close_href(), body))
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
    let command = format!("Run: `nullrouter accounts signin {} {}`", text("provider"), text("name"));
    html! {
        div class="provider-window__account" {
            div class="provider-window__account-head" {
                b class="provider-window__account-name" { (name(&text("name"))) }
                (status(&text("state_text")))
            }
            div class="provider-window__account-facts" {
                (kv("kind", html! { (text("kind")) }))
                (kv("order", html! { (a["order"].to_string()) }))
                (kv("priority", html! { (priority) }))
                (kv("secret", html! { (text("secret")) }))
                @if a["kind"] == "signin" {
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

/// `All`, then one link per kind any loaded plugin declares, each with this provider's count.
fn kind_filter(req: &Req, providers: &[Value], models: &[Value], chosen: Option<&str>) -> Markup {
    let kinds: BTreeSet<&str> =
        providers.iter().flat_map(|p| rows(&p["capabilities"])).filter_map(Value::as_str).collect();
    let path = req.path();
    let all = req.href(&path, &["kind"], &[]);
    html! {
        nav class="kind-filter" aria-label="Model kinds" {
            (kind_link(&all, "All", models.len(), chosen.is_none()))
            @for kind in kinds.iter().copied() {
                @let href = req.href(&path, &[], &[("kind", kind)]);
                @let count = models.iter().filter(|m| in_kind(m, kind)).count();
                (kind_link(&href, kind, count, chosen == Some(kind)))
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

fn kind_link(href: &str, label: &str, count: usize, current: bool) -> Markup {
    let class = if current { "kind-filter__item kind-filter__item--current" } else { "kind-filter__item" };
    html! {
        a class=(class) href=(href) aria-current=[current.then_some("true")] {
            (label) " " span class="kind-filter__count" { (count) }
        }
    }
}

/// A model with what `model <provider> <model>` prints for it.
fn model_row(m: &Value) -> Markup {
    let list = |k: &str| m[k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
    let fact = |label: &str, value: Option<String>| html! { @if let Some(v) = value { (kv(label, html! { (v) })) } };
    html! {
        div class="model-row" {
            div class="model-row__head" {
                code class="code model-row__id" { (m["model"].as_str().unwrap_or_default()) }
                @if let Some(k) = m["kind"].as_str() { (badge(Tone::Primary, k)) } @else { (badge(Tone::Default, "untyped")) }
            }
            div class="model-row__facts" {
                (fact("name", m["name"].as_str().map(str::to_owned)))
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

/// "Provider plugins": what `plugins list --community` lists, with the switches disabled.
fn side(ctx: &Ctx<'_>) -> Markup {
    let plugins = rows(ctx.json(ViewName::Plugins));
    let installed: Vec<&Value> = of(plugins, "community").filter(|r| r["status"] == "installed").collect();
    let not_installed = of(plugins, "community").filter(|r| r["status"] != "installed").count();
    let is_installed = |r: &Value| installed.iter().any(|i| i["id"] == r["id"]);
    let bundled: Vec<&Value> = of(plugins, "bundled").collect();
    let own: Vec<&Value> = of(plugins, "user").filter(|r| !is_installed(*r)).collect();
    // An installed community plugin is loaded as a user plugin: that row says how it loaded.
    let state_of = |r: &Value| -> String {
        of(plugins, "user")
            .find(|u| u["id"] == r["id"])
            .and_then(|u| u["status"].as_str())
            .or_else(|| r["status"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let sections = html! {
        p class="plugin-row__note" { (prose(SWITCH_HINT)) }
        (components::side_section(&format!("Bundled · {}", bundled.len()), html! {
            @for r in &bundled { (plugin_row(ctx, r, r["status"].as_str().unwrap_or_default(), false)) }
        }))
        (components::side_section(&format!("Installed from community · {}", installed.len()), html! {
            @for r in &installed { (plugin_row(ctx, r, &state_of(*r), true)) }
        }))
        @if !own.is_empty() {
            (components::side_section(&format!("Your own · {}", own.len()), html! {
                @for r in &own { (plugin_row(ctx, r, r["status"].as_str().unwrap_or_default(), false)) }
            }))
        }
        (components::side_section(&format!("Community · not installed: {not_installed} available."), html! {
            p { (prose(COMMUNITY_HINT)) }
            (disabled("Install", None, INSTALL_HINT))
        }))
    };
    components::side_panel("extension", "Provider plugins", sections)
}

/// One plugin: its source and state as `plugins list` shows them, the two
/// disabled switches, and for a community plugin the uninstall hint.
fn plugin_row(ctx: &Ctx<'_>, r: &Value, state: &str, uninstall: bool) -> Markup {
    let id = r["id"].as_str().unwrap_or_default();
    html! {
        div class="plugin-row" {
            span class="plugin-row__logo" { (components::logo(ctx.logos.href(id), id)) }
            div class="plugin-row__text" {
                b class="plugin-row__id" { (name(id)) }
                span class="plugin-row__source" { (r["set"].as_str().unwrap_or_default()) }
            }
            (status(state))
            div class="plugin-row__switches" {
                (disabled_switch("Enabled", SWITCH_HINT))
                (disabled_switch("Shown", SWITCH_HINT))
            }
            @if uninstall { (disabled("Uninstall", None, &UNINSTALL_HINT.replace("<id>", id))) }
        }
    }
}
